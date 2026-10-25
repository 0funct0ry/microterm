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

    # Menu items for the right-click menu of this pane (OSC 7717, SPEC section 10.9.7):
    #   mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...]
    #   mt-menu clear [ID]
    function __mt_b64
        printf '%s' $argv[1] | base64 | tr -d '\n\r'
    end

    function mt-menu
        switch "$argv[1]"
            case add
                set -e argv[1]
                set -l id ''
                set -l kind send
                set -l label ''
                while set -q argv[1]
                    switch $argv[1]
                        case --id
                            set id $argv[2]
                            set -e argv[1]
                            set -e argv[1]
                        case --ctl
                            set kind ctl
                            set -e argv[1]
                        case --spawn
                            set kind spawn
                            set -e argv[1]
                        case --
                            set -e argv[1]
                            break
                        case '*'
                            if test -z "$label"
                                set label $argv[1]
                                set -e argv[1]
                            else
                                echo "mt-menu: unexpected argument: $argv[1]" >&2
                                return 2
                            end
                    end
                end
                if test -z "$label"; or not set -q argv[1]
                    echo "usage: mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...]" >&2
                    return 2
                end
                if test -z "$id"
                    set id (printf '%s' $label | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-*//;s/-*$//')
                end
                set -l payload
                if test $kind = spawn
                    set -l parts
                    for w in $argv
                        set -a parts '"'(string replace -a '"' '\\"' -- (string replace -a '\\' '\\\\' -- $w))'"'
                    end
                    set payload '['(string join , $parts)']'
                else
                    set payload (string join ' ' (string escape --style=script -- $argv))
                end
                printf '\e]7717;menu;add;%s;%s;%s;%s\a' $id (__mt_b64 $label) $kind (__mt_b64 $payload)
            case clear
                if set -q argv[2]
                    printf '\e]7717;menu;clear;%s\a' $argv[2]
                else
                    printf '\e]7717;menu;clear\a'
                end
            case '*'
                echo "usage: mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...] | mt-menu clear [ID]" >&2
                return 2
        end
    end

    # Pane-scoped key bindings and palette entries (OSC 7723 and 7724, SPEC sections 6.4.10-11):
    #   mt-key add [--id ID] [--ctl] CHORD -- COMMAND [ARG...]     mt-key clear [ID]
    #   mt-palette add [--id ID] [--ctl] LABEL -- COMMAND [ARG...] mt-palette clear [ID]
    function __mt_pane_reg
        set -l ns $argv[1]
        set -l num $argv[2]
        set -l verb $argv[3]
        set -e argv[1..3]
        switch "$verb"
            case add
                set -l id ''
                set -l kind send
                set -l first ''
                while set -q argv[1]
                    switch $argv[1]
                        case --id
                            set id $argv[2]
                            set -e argv[1]
                            set -e argv[1]
                        case --ctl
                            set kind ctl
                            set -e argv[1]
                        case --
                            set -e argv[1]
                            break
                        case '*'
                            if test -z "$first"
                                set first $argv[1]
                                set -e argv[1]
                            else
                                echo "mt-$ns: unexpected argument: $argv[1]" >&2
                                return 2
                            end
                    end
                end
                if test -z "$first"; or not set -q argv[1]
                    echo "usage: mt-$ns add [--id ID] [--ctl] ARG -- COMMAND [ARG...]" >&2
                    return 2
                end
                if test -z "$id"
                    set id (printf '%s' $first | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-*//;s/-*$//')
                end
                set -l payload (string join ' ' (string escape --style=script -- $argv))
                if test $ns = key
                    printf '\e]%s;key;add;%s;%s;%s;%s\a' $num $id $first $kind (__mt_b64 $payload)
                else
                    printf '\e]%s;palette;add;%s;%s;%s;%s\a' $num $id (__mt_b64 $first) $kind (__mt_b64 $payload)
                end
            case clear
                if set -q argv[1]
                    printf '\e]%s;%s;clear;%s\a' $num $ns $argv[1]
                else
                    printf '\e]%s;%s;clear\a' $num $ns
                end
            case '*'
                echo "usage: mt-$ns add [--id ID] [--ctl] ARG -- COMMAND [ARG...] | mt-$ns clear [ID]" >&2
                return 2
        end
    end
    function mt-key
        __mt_pane_reg key 7723 $argv
    end
    function mt-palette
        __mt_pane_reg palette 7724 $argv
    end
end
