---
title: Recording & replay
description: Record a tab to an asciicast v2 file and play it back into a new tab.
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

## File layout

```json
{"version":2,"width":120,"height":34,"timestamp":1700000000,"env":{"TERM":"xterm-256color","SHELL":"/bin/zsh"}}
[0.012345,"o","hello\r\n"]
[1.5,"i","ls\r"]
```

Times are seconds since the recording started. Invalid UTF-8 from a program is replaced with
U+FFFD, as asciinema does. Resizes during a recording are not recorded in this version.
