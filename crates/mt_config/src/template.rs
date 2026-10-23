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
right_click = "context_menu"     # "context_menu" | "paste" | "none" (= context_menu.viewport.trigger)
hide_while_typing = true
scroll_multiplier = 3
focus_follows_mouse = false      # hovering a pane focuses it (splits)

[panes]
inactive_dim = 0.15              # 0.0-1.0 dim overlay on unfocused panes
focus_delay_ms = 0               # hover delay before focus_follows_mouse switches panes

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
# [[workspaces.tabs]]
# title = "server"
# cwd = "~/src/api"
# command = ["cargo", "watch", "-x", "run"]
# [[workspaces.tabs]]
# title = "shell"
# cwd = "~/src/api"

# [[ssh_domains]]
# name = "prod"
# host = "prod.example.com"

# [[serial_ports]]
# name = "arduino"
# port = "/dev/ttyUSB0"
# baud = 115200

[gpu]
prefer = "high_performance"      # "high_performance" | "low_power" (restart required)

[ctl]
capture_max_bytes = 1048576      # cap on output returned by `pane run` / `pane get-text` (tail kept)
status_max_len = 120             # `pane status` text is truncated to this many characters

[notifications]                  # for `ctl pane notify`
badge = true                     # attention badge on the tab until it is focused
toast = "always"                 # "always" | "background" (pane not visible) | "never"
desktop = "unfocused"            # "unfocused" | "always" | "never" (OS notification)

[terminal]                       # VT behaviour
osc_color_report = "16-bit"      # reply to OSC colour queries: "16-bit" | "8-bit" | "none"
osc_notifications = true         # let programs raise notifications with OSC 9
allow_shift_capture = true       # let programs make Shift-modified mouse events reach them (XTSHIFTESCAPE)

[sh]                             # `microterm sh`, the interactive control shell
color = "auto"                   # "auto" | "always" | "never" (NO_COLOR is always honoured)
history_send_text = false        # keep lines containing `send-text` in the history
confirm_prompts = true           # offer `Close 3 panes? [y/N]` when a close needs confirmation

[snippets]                       # snippets side panel + `{{ }}` templates (snippets.toml)
enabled = false                  # master switch; false = feature fully inert (no panel, no file read)
templates = false                # expand {{ }} templates; false = bodies are inserted literally
panel_width = 300                # px, 220-600; dragging the panel edge overrides it per window
panel_open_on_start = false      # open the panel in new windows (saved window state wins)
confirm_multiline = false        # ask before inserting a multi-line snippet
focus_terminal_after_insert = true
preview_lines = 2                # body lines shown under each title in the panel (0-5)

[recording]                      # recording mode, store and Recordings panel (SPEC §10.10)
enabled = false                  # master switch; false = feature fully inert (no recorder, pill, panel or rows)
capture_input = false            # also record keystrokes/paste (needed for the VHS exporter); opt-in
redact_no_echo = true            # drop input while the local PTY has ECHO off (password prompts)
max_duration_minutes = 60        # auto-stop a recording (0 = unlimited)
max_size_mb = 200                # auto-stop a recording at this size (0 = unlimited)
panel_width = 360                # px, 280-600
panel_open_on_start = false
confirm_delete = true

[recording.export]               # defaults for the export dialog and `ctl recording export`
default_format = "gif"           # "gif" | "vhs" | "cast" | any registered exporter id
output_dir = ""                  # "" = ask (save panel) in the GUI, current directory in ctl
open_when_done = false
strip_input = true               # cast: drop input events from exports
gif_fps = 10                     # 1-30
gif_idle_limit = 2.0             # seconds; pauses are shortened to this (0 = off)
gif_font_size = 14               # px
gif_scale = 1.0                  # 0.25-2.0
gif_chrome = "plain"             # "none" | "plain" | "macos"
gif_end_pause = 2.0              # seconds on the last frame
gif_max_size_mb = 50             # abort an export that would exceed this
vhs_type_gap_ms = 150            # keys closer than this become one Type
vhs_min_sleep_ms = 400           # shorter gaps emit no Sleep
vhs_sleep_cap_ms = 3000          # longest Sleep emitted

[menu_bar]                       # the macOS main menu bar
enabled = true                   # false keeps only the microterm menu (with Quit)

# Context menus (`viewport`, `tab`): add items, prune built-ins, change triggers.
# [context_menu.viewport]
# mode = "append"                # "append" | "prepend" | "replace"
# [[context_menu.viewport.items]]
# label = "git log -S {selection}"
# when = "selection"
# spawn = { command = ["git", "log", "-S", "{selection}"], cwd = "{cwd}", where = "split-right" }

[menu_security]
dangerous = ["rm -rf", "DROP ", "kubectl delete", "git push --force"]  # always confirm
confirm_panes = ["title:*prod*", "domain:*prod*"]  # confirm = "auto" always confirms here
trusted_dirs = []  # directories ("/dir" exact, "/dir/**" subtree) where shell-registered `spawn` items and project menu files (.microterm-menu.toml, .mt-menu.toml, .menu.toml, .p-menu.toml) work
allow_remote_items = false  # honour menu items registered from ssh-domain panes
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
