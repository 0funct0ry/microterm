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
fi
