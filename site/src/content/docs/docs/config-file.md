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

## Errors

- **Syntax and type errors** are errors. microterm keeps the last valid config.
- **Unknown keys** are warnings: the config still applies and the banner suggests the nearest
  key, for example `microterm.toml:12:5 — unknown key "fnt" (did you mean "font"?)`.
- **Out-of-range values** (`window.opacity = 2`, an unknown theme name) are errors.
