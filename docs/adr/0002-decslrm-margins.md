# ADR 0002 — DECSLRM / DECLRMM (left and right margins)

Status: accepted (M10.6), option (b): unsupported. Engine checked: `alacritty_terminal` 0.26.0.

## Question

Ghostty's VT reference lists `CSI Pl ; Pr s` (DECSLRM). It only means something while DECLRMM
(mode 69) is set. Should microterm (a) patch a vendored `alacritty_terminal` to add margins, or
(b) leave them unsupported?

## What margins would change in the engine

`alacritty_terminal` has no left or right margin state: the scroll region is a top/bottom
`Range<Line>` (`set_scrolling_region`) and every horizontal operation uses the whole row. Setting
margins changes all of these in `src/term/mod.rs` (0.26.0) and `src/grid/mod.rs`:

| Function | Change |
|---|---|
| `input` / `wrapline` | wrap at the right margin, not the last column; autowrap and the pending-wrap flag move with it |
| `linefeed`, `newline`, `reverse_index`, `scroll_up_relative`, `scroll_down_relative`, `Grid::scroll_up` / `scroll_down` | scroll only the columns between the margins (today they move whole rows with `rotate`) |
| `insert_blank_lines`, `delete_lines` (IL, DL) | operate on the margin columns only |
| `scroll_up`, `scroll_down` (SU, SD) | same as above |
| `insert_blank` (ICH), `delete_chars` (DCH), `erase_chars` (ECH) | stop at the right margin instead of the row end |
| `carriage_return`, `goto_col`, `move_forward`, `move_backward`, `put_tab`, `move_forward_tabs`, `move_backward_tabs` | clamp to the margins when the cursor is inside them, and CR goes to the left margin |
| `decaln`, `clear_line` | unchanged by the standard, but origin mode makes `CUP` relative to the margins |
| `swap_alt`, `resize`, `reset_state` | per-screen margin state, reset on resize and RIS |
| `set_mode` / `unset_mode` / `report_mode` | new mode 69, and DECRQM must report it |

`CSI s` also changes meaning: with mode 69 set it is DECSLRM, otherwise save-cursor, so the
parser (`vte::ansi`) would need to know the mode. `vte` is a separate crate, so the patch would
cover two vendored crates, not one.

## Decision: (b), unsupported

- It is a large, cross-cutting patch (about fifteen functions in two crates, plus the grid
  `rotate` fast paths) to a crate we otherwise take from crates.io at an exact version. Every
  upgrade would re-apply and re-review it.
- Almost no program uses it: it is a DEC feature of VT420 and later, used by a few full-screen
  editors for horizontal scrolling and by conformance suites (vttest).
- The M10.6 goal allows either outcome. Leaving it out keeps the engine unpatched and the replay
  synthesis unchanged.

Consequences:

- `CSI ? 69 $ p` (DECRQM) replies `CSI ? 69 ; 0 $ y` (mode not recognised); `CSI ? 69 h` is
  ignored.
- `CSI s` stays save-cursor. `CSI Pl ; Pr s` with parameters is not margin-setting.
- The conformance row for DECSLRM is `unsupported` and links here. The test cases for it pin the
  three observable behaviours above.

Revisit if a program we care about needs margins; the table above is the work list.
