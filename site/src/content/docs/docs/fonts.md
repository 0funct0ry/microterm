---
title: Fonts
description: Font family, ligatures, OpenType features, fallback chains, emoji and font size.
---

The terminal font is set in the `[font]` table and hot-reloads: change it and every window
re-measures its cells, resizes its PTYs and repaints.

```toml
[font]
family      = "JetBrains Mono"
size        = 13.0
line_height = 1.2              # multiplier of the font's natural line height
weight      = "regular"        # thin..black or 100..900
bold_weight = "bold"
ligatures   = true             # toggles calt/liga/dlig together
features    = ["zero", "ss02"] # extra OpenType features
fallback    = ["Symbols Nerd Font Mono", "Noto Sans CJK JP"]
```

`features` entries are four-character OpenType tags, optionally `tag=N` (for example
`"ss02=1"`). They are applied after the ligature toggle, so `"calt=0"` wins over
`ligatures = true`. A malformed tag is reported with its line and column.

## Bundled fonts

JetBrains Mono (the default, with ligatures) and Symbols Nerd Font Mono (MIT) ship inside the
binary. The default `fallback` list starts with Symbols Nerd Font Mono, so powerline and devicon
glyphs work out of the box. Any font installed on the system can be used by family name.

## Fallback chain

For each character microterm uses the first font in this order that has the glyph:

1. `font.family`
2. each entry of `font.fallback`, in order
3. the platform emoji font (Apple Color Emoji, Noto Color Emoji)
4. the platform's own last resort

Inspect it with `ls-fonts`:

```sh
microterm ls-fonts                       # the chain, plus a sample string
microterm ls-fonts --text "λ→🦀漢"       # one line per character: font and reason
```

The font is found by reading the `cmap` of each installed face, so `ls-fonts` needs no window.
The platform last resort is the first installed font that has the glyph, which approximates the
operating system's own choice.

## Ligatures

Ligatures are shaped across runs of ordinary text, so `==>`, `!=`, `<=` and `->` join in fonts
that provide them. A ligature that covers N cells is drawn across exactly N cells. When the
cursor sits inside a ligature, that run is shaped without ligatures so every cell stays
individually addressable; the ligature returns when the cursor moves away. Selection follows the
same rule once mouse selection ships.

Set `ligatures = false` to turn them off everywhere.

## Wide characters and emoji

East Asian wide characters and emoji take two cells. Glyphs from fallback fonts are centred in
their cells, and any glyph wider than its cells is scaled down to fit. Two regional indicators in
a row are drawn as one flag.

Cell widths come from the terminal engine's tables and there is no grapheme clustering:

- A zero-width joiner sequence such as a family emoji keeps one two-cell slot per emoji.
- A skin-tone modifier takes its own two cells after the base emoji.
- A variation selector (VS15/VS16) does not change the width.
- A flag is two regional indicators of one cell each, so it takes two cells.

### Flags and the zsh cursor

zsh's line editor counts every regional indicator as two cells, so it thinks a flag is four
cells wide. Terminals that count a flag as two disagree with it. The visible symptom is a
cursor that jumps to the wrong column after [zsh-autosuggestions](https://github.com/zsh-users/zsh-autosuggestions)
shows a suggestion containing a flag, for example after typing `ec` when your history has
`echo 🇯🇵🇫🇷`: the line is redrawn on top of itself. Alacritty has the same problem, and the
cause is zsh's width table, not the terminal.

If you use zsh and flags in your history, count flags the way zsh does:

```toml
[text]
wide_regional_indicators = false   # default
```

Set it to `true` and each regional indicator takes two cells, so a flag takes four and the
cursor lands where zsh expects. The flag is still drawn as one glyph, centred across its four
cells, and copying it gives back the flag without extra spaces.

Things to know:

- It is **off by default** because four cells is wider than other programs and terminals use.
  With it on, a program that counts a flag as two cells (most TUIs, `wcswidth` from libc,
  `ncurses`) will misplace the cursor instead.
- It applies to output received after the setting changes; existing lines are not rewritten.
  Config hot reload applies it to every open tab.
- It only affects regional indicators. Other emoji keep the engine's widths.
- Indicators inside window titles and other escape-sequence payloads are left untouched.
- Under the hood microterm writes one space after each indicator before the engine sees it
  and draws the indicator plus that space as one wide cell. A flag that ends exactly at
  the last column is the exception: the filler space wraps to the next line, so that flag
  shows as two narrow indicators.

## Bold and italic

If a family has no bold or italic face, the platform text system synthesises one.

## Changing the size

| Action | macOS | Linux |
|---|---|---|
| `IncreaseFontSize` | `cmd+=` | `ctrl+=` |
| `DecreaseFontSize` | `cmd+-` | `ctrl+-` |
| `ResetFontSize` | `cmd+0` | `ctrl+0` |

These change the size of the current window only (6 to 72 px), recompute the cell size and
resize the grid, and the shell receives `SIGWINCH`. Editing `font.size` in the config replaces
the window's own size.

See also: [Terminal API (VT) support](/docs/reference/terminal-api/) for the control sequences
microterm understands.
