---
title: Recording & replay
description: Record a pane to an asciicast v2 file or into the recordings store, and play it back into a new tab.
---

microterm records terminals in the [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/)
format, so recordings play in `asciinema play`, embed in web pages with the asciinema player and
diff well (one JSON line per event).

## Record a command

```sh
microterm record -o demo.cast -- cargo test
```

Runs the command in a new tab of the running instance and records until it exits. The recording
is armed before the command starts, so no output is lost. `--cols N --rows N` request a size
(the GUI uses the window's size).

## Record an existing pane

```sh
microterm ctl record --pane 3 -o session.cast            # until the pane closes or Ctrl-C
microterm ctl record --pane 3 -o session.cast --duration 30
```

Recording starts at the moment of the call; the header holds the pane's current size.

## Input is opt-in

By default only **output** is recorded. Pass `--input` to also record every key and paste sent to
the pane (`"i"` events). Leave it off unless you need it: input includes everything you type,
**passwords at a sudo prompt and tokens pasted into a login included**, and anything echoed back
is recorded as output anyway. Treat a recording made with `--input` like a keylogger's log and
review it before sharing.

## Replay

```sh
microterm ctl replay demo.cast [--speed 2] [--new-window]
```

Opens a new tab (or window) that plays the output with its original timing divided by `--speed`
and prints the pane id. The tab stays open after the replay so you can read the result; close it
like any tab. Replay feeds the recorded bytes to a normal terminal, so the final screen equals
the screen that was recorded.

See [Exporting recordings](/docs/exporting-recordings/) to convert a stored recording to a
trimmed asciicast or a VHS tape.

## File layout

```json
{"version":2,"width":120,"height":34,"timestamp":1700000000,"env":{"TERM":"xterm-256color","SHELL":"/bin/zsh"}}
[0.012345,"o","hello\r\n"]
[1.5,"i","ls\r"]
```

Times are seconds since the recording started. Invalid UTF-8 from a program is replaced with
U+FFFD, as asciinema does. Recordings made into the [store](#the-recordings-store) also hold
resize events (`"r"`, `"120x34"`) and markers (`"m"`); `ctl record` files do not record resizes.

## The recordings store

Recording mode keeps recordings in a per-user store instead of a path you choose. It is **off by
default**: with `recording.enabled = false` no recorder runs, the store directory is never read or
created, and every `ctl recording` verb exits `10`. Turn it on in the config:

```toml
[recording]
enabled = true
```

```sh
microterm ctl recording start --pane $MICROTERM_PANE   # prints the new recording id
microterm ctl recording marker --pane $MICROTERM_PANE --label "build done"
microterm ctl recording stop --pane $MICROTERM_PANE
microterm ctl recording list
microterm ctl recording replay r_20261022_141503_ab12cd
```

The store is `$XDG_DATA_HOME/microterm/recordings/` (default
`~/.local/share/microterm/recordings`, also on macOS), created `0700` when the first recording
starts. Each recording is two files with files `0600`:

```
r_20261022_141503_ab12cd.cast    # asciicast v2, the source of truth: plays in any asciicast tool
r_20261022_141503_ab12cd.json    # sidecar: title, duration, size, cols/rows, origin, markers, flags
```

The sidecar can always be rebuilt from the `.cast`. A missing or corrupt one is regenerated, and a
`.cast` you copy into the folder by hand shows up in `ctl recording list`.

- A pane has at most one recording at a time; several panes can record at once.
- A recording ends when you stop it, the pane's process exits, its tab or window closes, a limit is
  reached, the feature is turned off in the config (every running recording is saved first) or
  microterm quits.
- It keeps recording across SSH and serial reconnects. Time spent disconnected is not recorded, and the
  screen redraw the daemon sends after an SSH reconnect is not recorded again.
- **Crash safety.** Events stream to `<id>.cast.partial`, flushed every second. After a crash or
  `kill -9` the next `ctl recording list` completes the file (a truncated last line is dropped) and
  marks it `recovered`.

### Recording mode (keys, pill, tab dot)

With `recording.enabled = true`, `⌘⌥R` / `Ctrl+Alt+R` (`ToggleRecording`) starts and stops a
recording of the focused pane. The palette ("Start Recording" / "Stop Recording"), **Edit ▸ Start
Recording** on macOS, the pane context menu (**Start/Stop recording**) and `ctl recording
start|stop` do the same. `StartRecording`, `StopRecording` and `RecordingMarker` (no default key)
are also available as actions.

- **REC pill.** A red dot, `REC` and the elapsed time (`REC 00:42`; `REC +INPUT 00:42` when
  `capture_input` is on) at the top right of the pane, left of the Copy / Quick Select / Search
  pill. The dot pulses once a second unless the OS asks for reduced motion (set
  `MICROTERM_REDUCE_MOTION=1` to force a steady dot). Click the pill to stop.
- **Not modal.** Keys still reach the shell, `Esc` included; copy mode can be used while recording
  (output keeps recording).
- **Tab dot.** Every tab with a recording pane shows a 6 px red dot before its title; the tooltip
  reads `Recording pane 12 — 00:42`. It sits beside the tab color, status dot and badge.
- **Toasts.** Start, stop ("Saved ‹title› · 00:42 · 1.2 MB" with **Open panel** and **Export…**,
  which open the [Recordings panel](#the-recordings-panel) and the
  [export dialog](/docs/exporting-recordings/#the-export-dialog)), a reached limit, and
  "Recording stopped: disabled in config" when you turn the feature off.
- **One recorder per pane.** Starting a second one on the same pane only shows a toast; other
  panes can record at the same time. Closing the pane or tab saves its recording.

With the default config the key, palette and menu rows are absent or only show "Recording is
disabled — set `recording.enabled = true`" with an **Open config** button.

### The recordings panel

`⌘⇧Y` / `Ctrl+Shift+Y` (`ToggleRecordingsPanel`), **View ▸ Recordings**, the palette row
"Toggle recordings panel" and the toast's **Open panel** button open a side panel on the right
that lists the store. It behaves like the [snippets panel](/docs/snippets/): the first press
opens it and focuses the search field, a second press with focus elsewhere focuses it, a third
closes it, and `FocusRecordingsPanel` opens or focuses without ever closing. The right side holds
one panel at a time: opening Recordings closes Snippets and the other way round. Each remembers
its own width (drag the left edge; `recording.panel_width`, 280–600) and whether it was open
(`ui.json`; `recording.panel_open_on_start`). Turning `recording.enabled` off closes it.

The header shows the total size of the store, a record button for the focused pane and a `…` menu
with **Reveal store folder**, **Import .cast…** (copies a file into the store and rebuilds its
sidecar) and **Refresh**. The search box ranks recordings by title, origin, command, folder and
date. Without a query the list is grouped as **Recording now**, **Today**, **Yesterday** and
**Earlier** (days are UTC, like the ids). A row shows the title, `00:42 · 1.2 MB · 120×34`, the
age, a pulsing red dot while it is still being recorded and badges: `input`, `input not
redacted`, `ssh: host`, `serial`, `recovered`.

| Gesture | Effect |
|---|---|
| click, `↑` / `↓` | select |
| `Enter`, double-click | replay in a new tab (`⌥Enter` replays at 2×) |
| `⌘E` / `Ctrl+E`, **Export** | open the [export dialog](/docs/exporting-recordings/#the-export-dialog) |
| `F2` | rename in place; writes the sidecar title and the title in the `.cast` header |
| `Delete`, `Backspace` on an empty search | delete after a confirmation (`recording.confirm_delete = false` skips it) |
| right-click | Replay, Replay at 2×, Export…, Rename, Copy path, Reveal in Finder / file manager, Delete… |

A running recording offers only **Stop**: it cannot be replayed, renamed, exported or deleted
until it is saved. `Esc` clears the search, then returns focus to the terminal.

### Limits

`recording.max_duration_minutes` (default 60) and `recording.max_size_mb` (default 200) end a
recording and save it; `0` means unlimited.

### Privacy

- Recordings contain **everything that was on screen**, including secrets a command printed.
  They never leave the machine: nothing is uploaded or synced.
- **Input** is recorded only with `recording.capture_input = true` or `ctl recording start
  --input` (`--no-input` overrides the config). It is the bytes sent to the pane: keys, pasted
  text, `send-text` and snippet inserts.
- **Password prompts.** With `recording.redact_no_echo = true` (default), input is dropped while a
  program on a local pane has echo turned off and reads a whole line (`sudo`, `ssh`, `passwd`);
  one marker, `input paused (no echo)`, is written for each such stretch. This is best effort: a
  program that reads secrets in raw mode (some TUIs) cannot be detected. SSH and serial panes
  cannot be inspected and are recorded as typed; the metadata says `input_redacted = "n/a"`.
- Snippet `secret` values are never recorded as input, whatever the setting.
