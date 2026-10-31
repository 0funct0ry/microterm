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

<video src="/demos/theme-switcher.mp4" autoplay loop muted playsinline preload="metadata" aria-label="Choosing a theme from the palette with a live preview" style="width:100%;border-radius:8px"></video>

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

## Tab colors

Tab colors resolve through nine UI tokens, `tab_color_blue`, `tab_color_purple`, `tab_color_pink`,
`tab_color_red`, `tab_color_orange`, `tab_color_yellow`, `tab_color_green`, `tab_color_teal` and
`tab_color_gray`. A theme that omits them derives them from its ANSI palette: red is `ansi[1]`,
green `ansi[2]`, yellow `ansi[3]`, blue `ansi[4]`, purple `ansi[5]`, teal `ansi[6]`, gray
`ansi[8]`, orange is the mix of red and yellow and pink the mix of red and purple (50 % each).
Derived colors are shifted in lightness (hue kept) just enough to reach 3:1 contrast against the
tab bar for the 2 px bar and to keep the tab title at 4.5:1 on the tinted background.

Override a token per theme under `[ui]` (`tab_color_orange = "#f59e42"`), or for every theme with
`[tab_bar.palette]` in your config. `make themes` fails a theme whose nine colors miss those
contrast targets. A custom `#rrggbb` (from `ctl` or the config) is clamped the same way.
