---
title: Themes
description: Bundled themes, the theme selector and writing your own.
---

microterm ships 53 themes (Zed defaults and popular community palettes), each with its author
and licence recorded in `themes/LICENSES.md`. Only permissively licensed themes are bundled.

```sh
microterm themes list          # name, appearance, bundled or user
microterm themes list --json
microterm themes show "Nord"
```

## Choosing a theme

Set it in the config:

```toml
theme = "Catppuccin Mocha"
# or follow the OS appearance:
theme = { dark = "Tokyo Night", light = "Tokyo Night Day" }
```

Or open the **Select theme** command from the palette. Moving the selection previews each theme
live; **Enter** keeps it for this session and **Esc** reverts. To make a choice permanent, put it
in the config file.

## Your own themes

Put `*.toml` files in `themes/` next to your config file (`~/.config/microterm/themes/`). A user
theme replaces a bundled theme with the same name, and edits are picked up live.

```toml
name = "My Theme"
appearance = "dark"            # "dark" | "light"
author = "You"
license = "MIT"

[colors]
foreground = "#dce0e5"
background = "#282c33"
cursor = "#74ade8"
cursor_text = "#282c33"
selection_bg = "#74ade840"
ansi    = ["#282c33", "#d07277", "#a1c181", "#dec184", "#74ade8", "#b477cf", "#6eb4bf", "#dce0e5"]
brights = ["#525561", "#de8f94", "#bad6a1", "#e9d4a1", "#a3c8ef", "#cba2dd", "#9ac9d1", "#ffffff"]

[ui]                           # optional; derived from [colors] when absent
surface = "#2f343e"
border = "#464b57"
accent = "#74ade8"
```

When `[ui]` is missing the chrome is derived in the OKLCH colour space: `surface` is the
background shifted 4 % in lightness towards the foreground, `border` 12 %, `text_muted` is a
30 % mix of foreground and background, and `accent` is ANSI blue. Every UI token (`surface`,
`elevated_surface`, `border`, `border_variant`, `text`, `text_muted`, `text_placeholder`,
`element_hover`, `element_selected`, `accent`, `success`, `warning`, `error`) can be overridden.

## Rules for bundled themes

`make themes` fails if a theme lacks a `license`, uses a non-permissive licence, has fewer than
16 ANSI colours or a foreground/background contrast below 4.5:1. Themes that fail the licence
rule are dropped, not replaced silently: Zenburn is not bundled because it is GPL licensed.
