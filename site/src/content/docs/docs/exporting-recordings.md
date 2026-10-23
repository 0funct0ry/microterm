---
title: Exporting recordings
description: Convert a stored recording to a trimmed asciicast file or a VHS tape with `ctl recording export`.
---

Recordings in the [store](/docs/recording-and-replay/) can be converted with
`microterm ctl recording export`. Needs `recording.enabled = true`. Exporters are pure
converters: they read the stored `.cast` and its metadata and write one file, never starting a
process or touching the network.

```sh
microterm ctl recording export --list-formats
microterm ctl recording export ID --format cast -o short.cast --opt idle_limit=1
microterm ctl recording export ID --format vhs -o demo.tape
```

Without `-o` the file is `<title>.<ext>` in `recording.export.output_dir` (or the current
directory). An existing file is never overwritten without `-f`. `--json` prints
`{path, bytes, format, report}`. Exit `12` means the export is not possible for this recording
(for example `vhs` without input) or an `--opt` is unknown or out of range; the message says
which. Options, ranges and defaults for every format are in
[Export formats](/docs/reference/export-formats/).

## The export dialog

In the [Recordings panel](/docs/recording-and-replay/#the-recordings-panel) press `⌘E` /
`Ctrl+E`, click **Export**, pick **Export…** from a row's menu, run `ExportRecording` or click
**Export…** on the "Saved" toast. The dialog is built from the same exporter registry as
`ctl recording export`, so it lists every registered format with its own option fields (booleans
toggle with `Space`, enums with `←` / `→`, numbers and text are typed), validates them with the
same rules, and starts from the same `[recording.export]` defaults. A format that cannot export
this recording, such as `vhs` for a recording without input, is shown greyed out with the reason.

The output field defaults to `<title>.<ext>` in `recording.export.output_dir` (your home folder
when empty) and follows the chosen format until you edit it; **Browse…** opens the system save
panel. **Open when done** (`recording.export.open_when_done`) opens the file when the export
finishes. `Enter` exports, `Tab` moves between fields and `Esc` cancels. If the file exists, the
first `Enter` asks and the second replaces it.

Exports run on a background thread. Each one gets a progress line with a cancel button at the
bottom of the panel (a cancelled export leaves no partial file), several can run at once, and
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
