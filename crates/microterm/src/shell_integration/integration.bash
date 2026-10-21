# microterm shell integration for bash (OSC 133 prompt marks + OSC 7 working directory).
# Add to ~/.bashrc:   eval "$(microterm shell-integration bash)"
if [[ -n "${BASH_VERSION-}" && $- == *i* && -z "${__mt_integrated-}" ]]; then
  __mt_integrated=1

  __mt_osc7() {
    local p=${PWD//%/%25}
    p=${p// /%20}
    printf '\e]7;file://%s%s\a' "${HOSTNAME-}" "$p"
  }

  # First in PROMPT_COMMAND: report the status of the command that just ended.
  __mt_pre_prompt() {
    local s=$?
    printf '\e]133;D;%s\a' "$s"
    __mt_osc7
    __mt_ready=
    return "$s"
  }

  # Last in PROMPT_COMMAND: frameworks that rebuild PS1 on every prompt run before this, so
  # the prompt marks are put around whatever they produced.
  __mt_post_prompt() {
    local s=$?
    case $PS1 in
      *'133;A'*) ;;
      *) PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]' ;;
    esac
    __mt_ready=1
    return "$s"
  }

  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=(__mt_pre_prompt "${PROMPT_COMMAND[@]}" __mt_post_prompt)
  else
    PROMPT_COMMAND="__mt_pre_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND};__mt_post_prompt"
  fi

  if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
    # PS0 is printed after the command line was read and before it runs.
    PS0=$PS0'\e]133;C\a'
    PS0=${PS0//'\e'/$'\e'}
    PS0=${PS0//'\a'/$'\a'}
  else
    # bash < 4.4 has no PS0: the first DEBUG trap after a prompt is the command starting.
    __mt_debug() {
      # The prompt hooks themselves are not commands the user typed.
      if [[ $BASH_COMMAND == __mt_pre_prompt ]]; then
        __mt_ready=
        return
      fi
      if [[ -n "${__mt_ready-}" ]]; then
        __mt_ready=
        printf '\e]133;C\a'
      fi
    }
    trap '__mt_debug' DEBUG
  fi

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
