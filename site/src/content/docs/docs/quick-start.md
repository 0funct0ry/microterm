---
title: Quick start
description: Run microterm and use the live terminal.
---

Run microterm from source (installers arrive with v1.0.0):

```sh
make run
```

The first tab starts your shell: `shell` from the config, then `$SHELL`, then your passwd entry,
then `/bin/sh`. On macOS it is started as a login shell. Child processes see `TERM=xterm-256color`,
`COLORTERM=truecolor`, `TERM_PROGRAM=microterm` and `TERM_PROGRAM_VERSION`.

## Everyday keys

| Action | macOS | Linux |
|---|---|---|
| Paste | `⌘V` | `Ctrl+Shift+V` |
| Clear scrollback | `⌘K` | `Ctrl+Shift+K` |
| Scroll a page up / down | `Shift+PageUp` / `Shift+PageDown` | same |
| Scroll to top / bottom | `Shift+Home` / `Shift+End` | same |

Plain `Ctrl+letter` always reaches your shell on Linux, and `Option` acts as `Alt` (Meta) on
macOS. Typing while scrolled back returns to the live screen. The mouse wheel scrolls the
scrollback; in full-screen programs such as `less` or `vim` it sends arrow keys.

## Paste safety

Pasting several lines into a program that has not turned on bracketed paste asks for confirmation
first, because every newline would run as Enter. Programs that enable bracketed paste (modern
shells, vim, tmux) receive the text as one block with no prompt.

## Window title and resizing

Programs set the tab and window title with `OSC 0` / `OSC 2`. Resizing the window recomputes the
rows and columns, resizes the PTY and shows a small `cols×rows` indicator.

## What is rendered

- 16, 256 and 24-bit colour; bold, dim, italic, reverse, hidden, strikethrough.
- Underlines: single, double, curly, dotted and dashed (`CSI 4:x m`), with `CSI 58` colours.
- Block, beam and underline cursors (`DECSCUSR`), blinking, hollow when the window is unfocused.
- Box drawing, block elements, Braille and powerline separators are drawn by microterm itself so
  they join cleanly at any font size.
- Alternate screen, synchronized output (mode 2026), the Kitty keyboard protocol and a visual
  bell.

`scripts/sgr-test.sh` prints all of the above.

## Known gaps

- `SGR 21` is treated as "cancel bold" (use `4:2` for double underline) and `SGR 53` (overline)
  is not drawn: both come from the terminal engine.
- Selection and Copy arrive with the mouse work in M7; configuration with M3.
