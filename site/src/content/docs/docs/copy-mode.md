---
title: Copy mode
description: Select and copy text from the screen and scrollback with vi-style keys.
---

Copy mode puts a block cursor on the screen and lets you move through the whole scrollback
with the keyboard. Press **Cmd+Shift+X** (Linux: **Ctrl+Shift+X**) to enter it. The cursor starts
at the terminal cursor. While copy mode is active the pane receives no keyboard input, although
its output keeps rendering. The pill at the top right shows the mode (`COPY`, `VISUAL`,
`V-LINE`, `V-BLOCK`) and any count or key sequence you have started.

<video src="/demos/copy-mode.mp4" autoplay loop muted playsinline preload="metadata" aria-label="Copy mode moving through scrollback with Vim keys and copying a selection" style="width:100%;border-radius:8px"></video>

## Keys

| Keys | Motion or action |
|------|------------------|
| `h` `j` `k` `l`, arrows | character / line |
| `w` `b` `e`, `W` `B` `E` | word / WORD motions |
| `0` `^` `$` | line start / first non-blank / end |
| `g g`, `G` | top of scrollback / bottom |
| `H` `M` `L` | top / middle / bottom of the viewport |
| `Ctrl-u` `Ctrl-d` `Ctrl-b` `Ctrl-f` | half / full page |
| `{` `}` | previous / next blank-line-separated block |
| `[` `]` | previous / next prompt |
| `v` `V` `Ctrl-v` | character / line / block selection |
| `o` | swap the two ends of the selection |
| `y` | yank the selection and exit |
| `Y` | yank and stay in copy mode |
| `/` `?`, `n` `N` | search forward / backward, next / previous |
| `q`, `Esc` | exit (`Esc` first clears the selection) |

Prefix a motion with a number: `5j` moves five lines, `3w` three words, `12G` goes to line 12
of the buffer. Pressing `v` again (or `V`, `Ctrl-v` for their kinds) drops the selection;
pressing a different selection key switches kind and keeps the anchor.

Selections can extend into scrollback, and the view scrolls to keep the cursor visible. Rows
that were soft-wrapped by the terminal are joined when yanked, a block selection takes the same
columns from every row, and `selection.trim_trailing_whitespace` applies to the result.

### Searching

`/` and `?` open a prompt in the pill. Enter jumps to the next match from the cursor, wrapping
around at the ends; `n` repeats in the same direction and `N` reverses. The search is plain
text and ignores case unless the query has a capital letter. Combined with `v`, it extends a
selection to the next match.

### Prompts

`[` and `]` jump between shell prompts. They need your shell to mark prompts with the OSC 133
`A` sequence (shell integration in zsh, bash and fish prompts do this); without the marks they
do nothing.

## Changing keys

Add `[[copy_mode.keys]]` entries to rebind a key or `Disable` a default. Keys are written like
other bindings; `action` is one of the names below.

```toml
[[copy_mode.keys]]
key    = "ctrl+j"
action = "PageDown"

[[copy_mode.keys]]
key    = "q"
action = "Disable"
```

Action names: `MoveLeft`, `MoveRight`, `MoveUp`, `MoveDown`, `WordForward`, `WordBackward`,
`WordEnd`, `BigWordForward`, `BigWordBackward`, `BigWordEnd`, `LineStart`, `FirstNonBlank`,
`LineEnd`, `Top`, `Bottom`, `ViewTop`, `ViewMiddle`, `ViewBottom`, `HalfPageUp`, `HalfPageDown`,
`PageUp`, `PageDown`, `ParagraphPrev`, `ParagraphNext`, `PromptPrev`, `PromptNext`, `SelectChar`,
`SelectLine`, `SelectBlock`, `SwapEnds`, `Yank`, `YankStay`, `SearchForward`, `SearchBackward`,
`SearchNext`, `SearchPrev`, `Cancel`, `Exit`. An unknown name shows a warning and keeps the
default binding. Overrides are read when copy mode opens. A rebinding replaces single-key
defaults; the `g g` sequence is fixed.
