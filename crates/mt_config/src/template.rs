//! The commented default config: `microterm config show-defaults` prints it and
//! `microterm config edit` creates the file from it. Its uncommented values equal the schema
//! defaults (checked by a test), so it is always a valid config.

pub const DEFAULT_TOML: &str = r##"# microterm configuration. Every value below is the default; delete what you do not change.
# Edits are applied live while microterm is running. Reference: the "Config file" docs page.

shell = ""                       # empty = $SHELL, then /etc/passwd, then /bin/sh
term = "xterm-256color"
scrollback_lines = 10000
new_tab_cwd = "inherit"          # "inherit" | "home" | "<path>"
confirm_close = "process"        # "always" | "process" (only if a non-shell process runs) | "never"
startup_workspace = "default"

# theme = { dark = "One Dark", light = "One Light" }   # follow the OS appearance
theme = "One Dark"

[env]                            # extra env vars for new local panes
# EDITOR = "nvim"

[font]
family = "JetBrains Mono"
size = 13.0
line_height = 1.2
weight = "regular"
bold_weight = "bold"
ligatures = true
features = []
fallback = ["Symbols Nerd Font Mono"]

[window]
padding = { x = 8, y = 6 }
opacity = 1.0                    # 0.0..1.0 (background only)
blur = false                     # macOS: background blur when opacity < 1
decorations = "auto"             # "auto" | "client" | "server" | "none" (restart required)
initial_size = { cols = 120, rows = 34 }
remember_size = true

[tab_bar]
position = "top"                 # "top" | "bottom"
hide_if_single = false
title_format = "{title}"         # placeholders: {title} {process} {cwd_basename} {index}
show_index = false

[cursor]
style = "block"                  # "block" | "beam" | "underline"
blink = true
blink_interval_ms = 530

[text]
blink = false
min_contrast = 1.0               # >1 enforces WCAG-like contrast between fg/bg
wide_regional_indicators = false # true: flags take 4 cells like zsh counts them (fixes cursor jumps)

[scrollbar]
mode = "auto"                    # "auto" | "always" | "never"

[selection]
word_chars = "-_./~:@"
copy_on_select = false
trim_trailing_whitespace = true

[mouse]
right_click = "context_menu"     # "context_menu" | "paste" | "none"
hide_while_typing = true
scroll_multiplier = 3
# focus_follows_mouse = false    # accepted from M11 (splits)

[clipboard]
osc52 = "write"                  # "write" | "read-write" | "disabled"

[paste]
confirm_multiline = true

[bell]
mode = "visual"                  # "visual" | "audible" | "none"

[quick_select]
enabled = true                    # false turns quick select off
alphabet = "asdfqwerzxcvjklmiuopghtybn"
patterns = []                    # extra regexes appended to built-ins
disable_builtin = []             # e.g. ["ipv6", "number"]

[hyperlinks]
schemes = ["http", "https", "mailto", "file"]
modifier = "cmd"                 # "cmd" | "ctrl" | "none"

# [[hyperlink_rules]]
# regex = '\b([A-Z]{2,}-\d+)\b'
# format = "https://linear.app/acme/issue/$1"

# [[keys]]                       # appended to / override defaults; action = "Disable" unbinds
# key = "cmd+shift+r"
# action = "ReloadConfig"
#
# [[keys]]
# key = "alt+1"
# action = { ActivateTab = 1 }

# [[launch]]                     # entries for the launcher
# name = "htop"
# command = ["htop"]
# cwd = "~"

# [[workspaces]]                 # declarative startup workspaces
# name = "api"
# tabs = [ { title = "server", cwd = "~/src/api", command = ["cargo", "watch", "-x", "run"] },
#          { title = "shell",  cwd = "~/src/api" } ]

# [[ssh_domains]]
# name = "prod"
# host = "prod.example.com"

[gpu]
prefer = "high_performance"      # "high_performance" | "low_power" (restart required)

[ctl]
capture_max_bytes = 1048576      # cap on output returned by `pane run` / `pane get-text` (tail kept)
status_max_len = 120             # `pane status` text is truncated to this many characters

[notifications]                  # for `ctl pane notify`
badge = true                     # attention badge on the tab until it is focused
toast = "background"             # "background" | "always" | "never"
desktop = "unfocused"            # "unfocused" | "always" | "never" (OS notification)

[sh]                             # `microterm sh`, the interactive control shell
color = "auto"                   # "auto" | "always" | "never" (NO_COLOR is always honoured)
history_send_text = false        # keep lines containing `send-text` in the history
confirm_prompts = true           # offer `Close 3 panes? [y/N]` when a close needs confirmation
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_str, Config};

    #[test]
    fn template_equals_defaults_without_warnings() {
        let l = parse_str(DEFAULT_TOML).expect("template parses");
        assert_eq!(l.config, Config::default());
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    }
}
