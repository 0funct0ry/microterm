---
title: Environment variables
description: Variables microterm reads and exports.
---

## Exported to every pane

| Variable | Value |
|----------|-------|
| `TERM` | `xterm-256color` (config `term`) |
| `COLORTERM` | `truecolor` |
| `TERM_PROGRAM` | `microterm` |
| `TERM_PROGRAM_VERSION` | the microterm version |
| `MICROTERM_PANE` | the pane's id: the default for `microterm cli --pane` |
| `MICROTERM_UNIX_SOCKET` | the IPC socket of the instance that owns the pane: the default for every `microterm cli` command |

`MICROTERM_UNIX_SOCKET` is only used by clients. A new GUI instance always creates its own socket,
even when started from inside a pane.

## Read by microterm

| Variable | Used for |
|----------|----------|
| `MICROTERM_UNIX_SOCKET` | which instance `microterm cli`, `record` and a second `microterm` talk to |
| `MICROTERM_PANE` | default `--pane` |
| `MICROTERM_CONFIG` | path of the config file |
| `XDG_RUNTIME_DIR` | socket directory on Linux (`$XDG_RUNTIME_DIR/microterm/`) |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | config and state locations |
| `VISUAL`, `EDITOR` | editor for *Open config* |
| `SHELL` | the shell, when config `shell` is empty |
