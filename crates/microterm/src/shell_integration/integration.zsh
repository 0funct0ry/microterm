# microterm shell integration for zsh (OSC 133 prompt marks + OSC 7 working directory).
# Add to ~/.zshrc:   eval "$(microterm shell-integration zsh)"
if [[ -o interactive && -z "${__mt_integrated-}" ]]; then
  typeset -g __mt_integrated=1
  autoload -Uz add-zsh-hook

  __mt_precmd() {
    local s=$?
    printf '\e]133;D;%s\a' "$s"
    local p=${PWD//\%/%25}
    p=${p// /%20}
    printf '\e]7;file://%s%s\a' "${HOST-}" "$p"
  }

  __mt_preexec() {
    printf '\e]133;C\a'
  }

  # Runs after the other precmd hooks, which may rebuild PS1 (themes do).
  __mt_wrap_prompt() {
    if [[ $PS1 != *'133;A'* ]]; then
      PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
    fi
  }

  add-zsh-hook precmd __mt_precmd
  add-zsh-hook preexec __mt_preexec
  add-zsh-hook precmd __mt_wrap_prompt

  # Menu items for the right-click menu of this pane (OSC 7717, SPEC section 10.9.7):
  #   mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...]
  #   mt-menu clear [ID]
  # The default kind types the command (quoted) into this pane followed by Enter. `--ctl`
  # runs `microterm ctl COMMAND` for this pane; `--spawn` opens it in a split (trusted
  # directories only). Items vanish when the shell exits.
  __mt_b64() { printf '%s' "$1" | base64 | tr -d '\n\r'; }
  __mt_json() {
    local out='[' sep='' w
    for w in "$@"; do
      w=${w//\\/\\\\}
      w=${w//\"/\\\"}
      out+="$sep\"$w\""
      sep=,
    done
    printf '%s]' "$out"
  }
  mt-menu() {
    case ${1-} in
      add)
        shift
        local id='' kind=send label=''
        while (( $# )); do
          case $1 in
            --id) id=${2-}; shift; shift ;;
            --ctl) kind=ctl; shift ;;
            --spawn) kind=spawn; shift ;;
            --) shift; break ;;
            *)
              if [[ -z $label ]]; then
                label=$1; shift
              else
                echo "mt-menu: unexpected argument: $1" >&2; return 2
              fi ;;
          esac
        done
        if [[ -z $label || $# -eq 0 ]]; then
          echo "usage: mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...]" >&2
          return 2
        fi
        if [[ -z $id ]]; then
          id=$(printf '%s' "$label" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-*//;s/-*$//')
        fi
        local payload
        if [[ $kind == spawn ]]; then
          payload=$(__mt_json "$@")
        else
          payload=$(printf '%q ' "$@")
          payload=${payload% }
        fi
        printf '\e]7717;menu;add;%s;%s;%s;%s\a' "$id" "$(__mt_b64 "$label")" "$kind" "$(__mt_b64 "$payload")"
        ;;
      clear)
        printf '\e]7717;menu;clear%s\a' "${2:+;$2}"
        ;;
      *)
        echo "usage: mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...] | mt-menu clear [ID]" >&2
        return 2
        ;;
    esac
  }

  # Pane-scoped key bindings and palette entries (OSC 7723 and 7724, SPEC sections 6.4.10-11):
  #   mt-key add [--id ID] [--ctl] CHORD -- COMMAND [ARG...]     mt-key clear [ID]
  #   mt-palette add [--id ID] [--ctl] LABEL -- COMMAND [ARG...] mt-palette clear [ID]
  # They act only on this pane, only while it has focus, and never override your own or a
  # built-in key. `send` (default) types the quoted command plus Enter; `--ctl` runs
  # `microterm ctl COMMAND` for this pane. Both vanish when the shell exits.
  __mt_pane_reg() {
    local ns=$1 num=$2 verb=$3; shift 3
    case $verb in
      add)
        local id='' kind=send first=''
        while (( $# )); do
          case $1 in
            --id) id=${2-}; shift; shift ;;
            --ctl) kind=ctl; shift ;;
            --) shift; break ;;
            *)
              if [[ -z $first ]]; then
                first=$1; shift
              else
                echo "mt-$ns: unexpected argument: $1" >&2; return 2
              fi ;;
          esac
        done
        if [[ -z $first || $# -eq 0 ]]; then
          echo "usage: mt-$ns add [--id ID] [--ctl] ${ns/key/CHORD} -- COMMAND [ARG...]" >&2
          return 2
        fi
        if [[ -z $id ]]; then
          id=$(printf '%s' "$first" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-*//;s/-*$//')
        fi
        local payload
        payload=$(printf '%q ' "$@")
        payload=${payload% }
        if [[ $ns == key ]]; then
          printf '\e]%s;key;add;%s;%s;%s;%s\a' "$num" "$id" "$first" "$kind" "$(__mt_b64 "$payload")"
        else
          printf '\e]%s;palette;add;%s;%s;%s;%s\a' "$num" "$id" "$(__mt_b64 "$first")" "$kind" "$(__mt_b64 "$payload")"
        fi
        ;;
      clear)
        printf '\e]%s;%s;clear%s\a' "$num" "$ns" "${1:+;$1}"
        ;;
      *)
        echo "usage: mt-$ns add [--id ID] [--ctl] ARG -- COMMAND [ARG...] | mt-$ns clear [ID]" >&2
        return 2
        ;;
    esac
  }
  mt-key() { __mt_pane_reg key 7723 "$@"; }
  mt-palette() { __mt_pane_reg palette 7724 "$@"; }

  # Collapsible output and semantic links (OSC 7721 and 7726, SPEC sections 6.4.12-13):
  #   mt-fold begin ID TITLE     mt-fold end ID
  #   mt-link add ID REGEX TEMPLATE   mt-link clear [ID]
  # Folding is display only (copy, search and get-text still see every line). A link rule turns
  # text matching REGEX into a link to TEMPLATE ($1..$9); the real target shows in the tooltip.
  mt-fold() {
    case ${1-} in
      begin) [[ $# -eq 3 ]] || { echo "usage: mt-fold begin ID TITLE | mt-fold end ID" >&2; return 2; }
        printf '\e]7721;fold;begin;%s;%s\a' "$2" "$(__mt_b64 "$3")" ;;
      end) [[ $# -eq 2 ]] || { echo "usage: mt-fold begin ID TITLE | mt-fold end ID" >&2; return 2; }
        printf '\e]7721;fold;end;%s\a' "$2" ;;
      *) echo "usage: mt-fold begin ID TITLE | mt-fold end ID" >&2; return 2 ;;
    esac
  }
  mt-link() {
    case ${1-} in
      add) [[ $# -eq 4 ]] || { echo "usage: mt-link add ID REGEX TEMPLATE | mt-link clear [ID]" >&2; return 2; }
        printf '\e]7726;link;add;%s;%s;%s\a' "$2" "$(__mt_b64 "$3")" "$(__mt_b64 "$4")" ;;
      clear) printf '\e]7726;link;clear%s\a' "${2:+;$2}" ;;
      *) echo "usage: mt-link add ID REGEX TEMPLATE | mt-link clear [ID]" >&2; return 2 ;;
    esac
  }
fi
