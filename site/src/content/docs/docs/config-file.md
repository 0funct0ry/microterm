---
title: Config file
description: Where microterm reads its configuration and what you can set.
---

microterm reads one TOML file. A missing file means all defaults, so you only write what you
change.

## Location

The first match wins:

1. `--config <path>`
2. `$MICROTERM_CONFIG`
3. `$XDG_CONFIG_HOME/microterm/microterm.toml`
4. `~/.config/microterm/microterm.toml` (also used on macOS)

```sh
microterm config path           # print the file microterm uses
microterm config edit           # create it from the commented template, then open your editor
microterm config check          # validate it (same checks as a hot reload)
microterm config show-defaults  # print every key with its default value
```

`config edit` uses `$VISUAL`, then `$EDITOR`, then the system opener (`open` / `xdg-open`).
`config check` exits with status 1 when the file is invalid and prints `file:line:column — message`
for each problem.

## Example

```toml
theme = { dark = "Tokyo Night", light = "Tokyo Night Day" }
scrollback_lines = 50000

[font]
family = "JetBrains Mono"
size = 14

[window]
padding = { x = 12, y = 8 }
opacity = 0.95

[cursor]
style = "beam"
blink = false

[[keys]]
key = "cmd+shift+r"
action = "ReloadConfig"

[[keys]]
key = "cmd+t"
action = "Disable"          # unbind a default
```

## What is applied today

| Keys | Effect |
|---|---|
| `theme` | colours of the terminal and the UI |
| `font.*` | cell metrics, ligatures, OpenType features and the fallback chain; see [Fonts](/docs/fonts/) |
| `window.padding`, `opacity`, `blur`, `initial_size` | viewport padding, background transparency, size of new windows |
| `cursor.style`, `blink`, `blink_interval_ms` | default cursor (programs can still change it with DECSCUSR) |
| `scrollback_lines`, `shell`, `env`, `term`, `new_tab_cwd` | new panes only |
| `tab_bar.position`, `hide_if_single` | tab bar placement and visibility |
| `scrollbar.mode`, `bell.mode`, `paste.confirm_multiline`, `clipboard.osc52`, `mouse.scroll_multiplier` | behaviour |
| `[[keys]]` | keybindings |
| `ctl.capture_max_bytes` | cap on the output `pane run` and `pane get-text` return (default 1048576; the tail is kept); read when a request arrives |
| `ctl.status_max_len` | `pane status` and `pane notify` text is cut to this many characters (default 120); read when a request arrives |
| `notifications.badge`, `toast`, `desktop` | how `pane notify` surfaces: tab badge, toast (`background`, `always`, `never`) and OS notification (`unfocused`, `always`, `never`); see [Notifications & status](/docs/notifications-and-status/) |
| `terminal.osc_color_report` | reply to OSC colour queries (OSC 4, 5, 10–19, 21): `"16-bit"` (default, `rgb:RRRR/GGGG/BBBB`), `"8-bit"` (`rgb:RR/GG/BB`) or `"none"` (no reply) |
| `terminal.osc_notifications` | `true` (default): a program can raise a notification with OSC 9; `false` ignores it. The text is program-controlled, so it is sanitised, cut to `ctl.status_max_len` and limited to 5 per second per pane |
| `terminal.allow_shift_capture` | `true` (default): a program can turn on XTSHIFTESCAPE so Shift-modified mouse events reach it; see [Mouse & selection](/docs/mouse-and-selection/) |

These three apply to sequences received after a reload; see [Terminal API (VT) support](/docs/reference/terminal-api/).

Every other key in the schema is accepted and validated now, and starts working in the release
that implements its feature.

`bell.mode = "audible"` plays a system sound through `afplay` (macOS) or `canberra-gtk-play`
(Linux) when available.

## Keybindings

`key` is modifiers joined by `+` and then a key: `cmd+shift+p`, `ctrl+alt+k`, `alt+1`, `f5`,
`ctrl+pageup`. Modifiers are `cmd` (alias `super`), `ctrl`, `alt` (alias `opt`) and `shift`. An
`action` is a name (`"ReloadConfig"`), a name with an argument (`{ ActivateTab = 3 }`, tabs count
from 1) or `"Disable"` to remove a default binding. User bindings win over defaults on the same
keystroke. An unknown action name is skipped and shown in the banner.

## Context menus

`[context_menu.<menu>]` tables add items to the right-click menus (`viewport`, `tab`), prune or
reorder the built-ins and set the trigger; `[[menu_groups.<name>.items]]` are reusable groups and
`[menu_security]` lists what always needs confirmation. Menu changes apply the next time a menu
opens. See [Context menus](/docs/context-menus/) and the
[menu reference](/docs/reference/context-menus/).

## Errors

- **Syntax and type errors** are errors. microterm keeps the last valid config.
- **Unknown keys** are warnings: the config still applies and the banner suggests the nearest
  key, for example `microterm.toml:12:5 — unknown key "fnt" (did you mean "font"?)`.
- **Out-of-range values** (`window.opacity = 2`, an unknown theme name) are errors.
- **Context-menu mistakes** (an unknown menu id, a placeholder or `when` key the menu does not
  have, an unparsable `ctl` string, an unknown action or icon, more than 64 items) are errors with
  the item's path, for example `context_menu.tab.items[2]: placeholder {selection} is not
  available in menu "tab"`.
