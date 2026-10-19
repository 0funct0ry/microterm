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
fi
