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
fi
