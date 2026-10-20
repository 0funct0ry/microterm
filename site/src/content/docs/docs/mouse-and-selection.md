---
title: Mouse & selection
description: Mouse reporting, selecting and copying text, right-click behaviour and pointer shapes.
---

## Applications that use the mouse

When a program turns on mouse tracking (vim with `:set mouse=a`, htop, tmux, less) microterm
sends it clicks, drags and wheel events using the encoding the program asked for: legacy
X10-style, SGR (1006) or SGR-pixels (1016), with click (1000), drag (1002) or motion (1003)
tracking.

Hold **Shift** to bypass reporting and select text instead, in any program. A program can ask
for the opposite with XTSHIFTESCAPE (`CSI > 1 s`): Shift-modified mouse events then go to it,
and `CSI > 0 s` restores the bypass. This is per pane, ends with a terminal reset, and only
works while `terminal.allow_shift_capture` is true (the default).

In the alternate screen, when the program has not enabled mouse tracking, the wheel is
translated to Up/Down arrow keys (like `less` and `man` expect).

## Selecting text

| Gesture | Selects |
|---|---|
| Drag | Characters |
| Double-click | A word |
| Triple-click | A whole line |
| Alt/Option + drag | A rectangular block |
| Shift + click | Extends the current selection |

Dragging above or below the grid scrolls through scrollback and keeps extending the selection.
Typing clears the selection.

A word is letters, digits and the characters in `selection.word_chars`, so paths and URLs
select as one word by default.

```toml
[selection]
word_chars = "-_./~:@"          # extra characters that belong to a word
copy_on_select = false          # copy to the clipboard as soon as you release the mouse
trim_trailing_whitespace = true # strip blanks at the end of each copied line
```

On Linux every selection also sets the primary selection, and middle-click pastes it
(`PastePrimary`). macOS has no primary selection. Use `Copy` and `Paste`.

## Right-click

```toml
[mouse]
right_click = "context_menu"    # "context_menu" | "paste" | "none"
hide_while_typing = true
scroll_multiplier = 3
```

The context menu offers Copy, Paste, Select all, Open link, Copy link, Search for selection and
Clear scrollback. Entries that do not apply (no selection, no link under the pointer) are
dimmed.

## Pointer shapes

An I-beam over the terminal, an arrow while a program is capturing the mouse, a pointing hand
over a link while the activation modifier is held, and the pointer disappears while you type
(`mouse.hide_while_typing`) until you move it.

A program can pick another shape with OSC 22 (`printf '\e]22;pointer\e\\'`; an empty name
resets). Names follow the X11 cursor list used by xterm and Kitty, for example `pointer`, `text`,
`crosshair`, `grab`, `grabbing`, `not-allowed`, `ew-resize`, `ns-resize` and `col-resize`. A name
with no equivalent cursor shows the arrow. A link under the pointer wins over OSC 22, and the
shape ends when the program turns mouse reporting off, on a terminal reset and when the pane
exits.

See also: [Terminal API (VT) support](/docs/reference/terminal-api/).
