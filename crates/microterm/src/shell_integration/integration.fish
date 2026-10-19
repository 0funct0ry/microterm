# microterm shell integration for fish (OSC 133 prompt marks + OSC 7 working directory).
# Add to ~/.config/fish/config.fish:   microterm shell-integration fish | source
if status is-interactive; and not set -q __mt_integrated
    set -g __mt_integrated 1

    function __mt_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end

    function __mt_postexec --on-event fish_postexec
        set -l s $status
        printf '\e]133;D;%s\a' $s
    end

    function __mt_osc7 --on-variable PWD
        printf '\e]7;file://%s%s\a' (hostname) (string escape --style=url -- $PWD)
    end
    __mt_osc7

    if functions -q fish_prompt
        functions -c fish_prompt __mt_original_prompt
        function fish_prompt
            printf '\e]133;A\a'
            __mt_original_prompt
            printf '\e]133;B\a'
        end
    end
end
