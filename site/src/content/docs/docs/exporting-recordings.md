---
title: Exporting recordings
description: Convert a stored recording to an animated GIF, a trimmed asciicast file or a VHS tape, from the panel or with `ctl recording export`.
---

Recordings in the [store](/docs/recording-and-replay/) can be converted with
`microterm ctl recording export`. Needs `recording.enabled = true`. Exporters are pure
converters: they read the stored `.cast` and its metadata and write one file, never starting a
process or touching the network.

```sh
microterm ctl recording export --list-formats
microterm ctl recording export ID --format cast -o short.cast --opt idle_limit=1
microterm ctl recording export ID --format vhs -o demo.tape
microterm ctl recording export ID --format gif -o demo.gif --opt fps=15 --opt chrome=macos
```

Without `-o` the file is `<title>.<ext>` in `recording.export.output_dir` (or the current
directory). An existing file is never overwritten without `-f`. `--json` prints
`{path, bytes, format, report}`. Exit `12` means the export is not possible for this recording
(for example `vhs` without input) or an `--opt` is unknown or out of range; the message says
which. Options, ranges and defaults for every format are in
[Export formats](/docs/reference/export-formats/).

## The export dialog

In the [Recordings panel](/docs/recording-and-replay/#the-recordings-panel) press `⌘E` /
`Ctrl+E`, pick **Export…** from a row's right-click menu, run `ExportRecording` or click
**Export…** on the "Saved" toast. The dialog is built from the same exporter registry as
`ctl recording export`, so it lists every registered format with its own option fields (booleans
toggle with `Space`, enums with `←` / `→`, numbers and text are typed), validates them with the
same rules, and starts from the same `[recording.export]` defaults. A format that cannot export
this recording, such as `vhs` for a recording without input, is shown greyed out with the reason.

Options are laid out in two panes so the dialog fits on screen: for GIF, *Appearance* (theme,
window style, backdrop) on the left and *Text and timing* on the right. The theme is picked from
a list with a colour preview, the window style (`chrome`) is a three-way switch, and the margin
colour offers *Auto*, a row of preset swatches and a hex field; the line under the controls
explains the focused option.

Text fields work like any editor field: click to place the caret, drag or double / triple click
to select, and `Ctrl`/`⌘` + `A`, `C`, `X`, `V`.

The output file starts empty. The **home** button fills `<your home folder>/<recording
name>.<ext>` (and the extension then follows the chosen format), **browse** opens the system save
panel, and you can type a path. If `recording.export.output_dir` is set the field starts with
`<output_dir>/<recording name>.<ext>` instead. **Open when done** (`recording.export.open_when_done`) opens the file when the export
finishes. `Enter` exports, `Tab` moves between fields and `Esc` cancels. If the file exists, the
first `Enter` asks and the second replaces it.

Exports run on a background thread. Each one gets a card at the bottom of the panel with the
percentage, a progress bar and a **Cancel** button (the panel has no other footer; a cancelled export leaves no partial file), several can run at once, and
the result is a toast with **Reveal** and **Open**.

## asciicast (`cast`)

A verified copy of the stored file, optionally transformed: `trim_start` / `trim_end` (seconds
dropped from each end), `idle_limit` (cap pauses, as asciinema does), `title`, and `strip_input`.
`strip_input` defaults to `recording.export.strip_input` (on), so a shared `.cast` does not leak
keystrokes. With nothing to change the output is byte-identical to the stored file.

## VHS tape (`vhs`)

Writes a [VHS](https://github.com/charmbracelet/vhs) tape that retypes the session. A tape is an
input script, so this needs a recording made with input (`capture_input = true` or
`recording start --input`).

| Recording | Tape |
|---|---|
| header | `Output`, `Set Shell`, `Set FontSize`, `Set FontFamily`, `Set Width`/`Set Height`, `Set Padding`, `Set Theme` (only for a theme VHS knows, otherwise a comment) |
| printable keys closer than `vhs_type_gap_ms` (150) | `Type@<mean gap>ms "…"` (a single key is a plain `Type`) |
| Enter, Backspace, Tab, Escape | `Enter`, `Backspace`, `Tab`, `Escape` |
| arrows, Home, End, PageUp/PageDown, Delete | `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Delete` (CSI and SS3 forms) |
| `Ctrl+A`…`Ctrl+Z` | `Ctrl+A`…`Ctrl+Z` |
| bracketed paste | one `Type` per pasted line, `Enter` between lines |
| gap above `vhs_min_sleep_ms` (400) | `Sleep <n>ms`, at most `vhs_sleep_cap_ms` (3000) |
| input paused (echo off) marker | `Hide` … `Show` with a comment where the hidden input goes |
| mouse reports, focus reports, Alt+key, unknown sequences | dropped and listed in the report |

Limits:

- The tape records what to type, not what the shell printed; VHS regenerates the output, which
  may differ from the original session.
- VHS strings have no escapes, so the exporter picks `"`, `'` or `` ` `` per string, whichever
  the text lacks, and splits text that contains all three.
- Pixel size is computed from the grid: width = columns × 0.6 × font size, height = rows ×
  ⌈1.2 × font size⌉, plus 20 px padding on each side. Adjust `font_size` if VHS wraps lines.
- `Set Shell` uses the recorded process when it is a known shell, otherwise `bash`.
- Passwords typed at a no-echo prompt are not in the recording; the tape marks the spot.
- `Output` is `<name>.gif` after the tape's file name; override it with `--opt output=NAME`.

## Animated GIF (`gif`)

Replays the recording through a headless terminal and draws every frame with a built-in software
renderer, so it needs no window and works the same over `ctl`, in CI and on old recordings. The
same recording and options always give a byte-identical file.

What it draws: your theme (the one the recording was made with, or `theme=` with any bundled or
user theme), the bundled JetBrains Mono in regular, bold, italic and bold-italic, the 16 ANSI,
256 and truecolor palettes, dim, reverse video, underline and strikethrough, the block, bar or
underline cursor, double-width cells, and box-drawing and block characters drawn as shapes so
they join. `chrome` adds a frame: `macos` (default: rounded border, the recording's title and traffic
lights), `plain` (the same without the lights) or `none`. By default the window floats on a darker backdrop
(`margin`) with smooth rounded corners and a soft shadow, and is rendered at 2× so text stays
sharp on Retina screens.

Options (defaults from `[recording.export]` where a `gif_*` key exists):

| Option | Default | Meaning |
|---|---|---|
| `fps` | 10 (1-30) | Frames per second the replay is sampled at |
| `speed` | 1.0 | Playback speed; `2` plays twice as fast |
| `idle_limit` | 2.0 s | Longer pauses are shortened to this (`0` = keep them) |
| `font_size` | 16 px | Font size before `scale` |
| `line_height` | 1.1 (0.8-2.0) | Multiplies the font's line height |
| `scale` | 2.0 (0.25-3.0) | Multiplies every pixel size; 2 is sharp on Retina screens |
| `padding` | 16 px | Space around the grid |
| `margin` | 32 px | Backdrop around the window; `0` = none, corners transparent |
| `margin_fill` | theme shade | Backdrop colour as `#rrggbb` |
| `shadow` | on | Soft drop shadow under the window (needs a margin) |
| `cursor` | on | Draw the cursor |
| `crop` | on | Show only the part of the terminal the recording uses (see below) |
| `loop` | 0 | How many times the GIF plays; `0` = forever |
| `end_pause` | 2.0 s | How long the last frame stays |
| `title` | recording's name | Text shown in the window's title bar (prefilled in the dialog, editable) |
| `theme` | recording's | Any bundled or user theme name (a list in the dialog) |
| `chrome` | `macos` | `none`, `plain` or `macos` |
| `trim_start` / `trim_end` | 0 | Seconds cut from each end |
| `max_size_mb` | 50 | Stop with an error instead of writing a bigger file |

A frame is written only when the screen changed, and only the changed rectangle is stored, so a
mostly idle prompt costs almost nothing.

Not rendered in this version:

- Fonts other than the bundled JetBrains Mono (the export never looks up system fonts). Characters
  it has no glyph for, such as CJK, are drawn as a `□` box of the right width. The report counts
  them.
- Ligatures and inline images (graphics protocols).

Colour emoji (📦 🦀 ✅) are drawn in colour from the bundled Noto Color Emoji, so they look the
same on every machine but not like Apple's emoji. Only the first emoji of a sequence is drawn:
skin tones, ZWJ families and flags show their base emoji.

With `crop` on (the default) the window is cut to the cells the recording ever uses (at least 40
columns by 3 rows, never more than the recorded window), so a 238×56 terminal that only shows a
prompt exports as a tight window with large text. Without it the whole recorded window is drawn.
The font size and `scale` set how many pixels each cell takes: they resize the whole picture, so
with the full window shown they cannot make the text look bigger relative to the frame; cropping
does.

The picture size is the terminal size times the font size times `scale`: a 238×56 recording at the
default `scale=2` is 4714×2889 pixels, so large terminals export much faster and give smaller files
with `scale=1`. Only the rows that changed are redrawn and encoded, so mostly static sessions are
cheap.

Size tips: a full screen of scrolling output changes every pixel, so fast-scrolling sessions grow
fastest. Lower `fps` (5 is fine for shell demos), use `scale=1` (a quarter of the pixels of the
default), keep `idle_limit` on and trim the ends.

Corners: with a margin the rounded corners are smooth against `margin_fill`; set `margin_fill` to
your page colour (for example `#ffffff` for a light README) to make the backdrop disappear. With
`margin=0` the corners are transparent instead, but GIF transparency is on/off, so they are not
smoothed. If an export stops at `max_size_mb`, the message says so; raise it only if you need to.
