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
| `MICROTERM_PANE` | the pane's id: the default pane for `microterm ctl` |
| `MICROTERM_TAB` | the id of the tab holding the pane: the default tab for `microterm ctl` |
| `MICROTERM_WINDOW` | the id of the window holding the pane: the default window for `microterm ctl` |
| `MICROTERM_UNIX_SOCKET` | the IPC socket of the instance that owns the pane: the default for every `microterm ctl` command |

The four `MICROTERM_*` variables are exported into **local** panes only. Panes of an
[SSH domain](/docs/ssh-domains/) never get them: the local socket is not reachable from a remote
host, and it is never forwarded over SSH. The ids are the ones at the time the pane started; a
pane's tab and window do not change today.

## Exported to context-menu programs

A [context-menu](/docs/context-menus/) `spawn` item runs its program with these as well (each
present only when it has a value, with control characters removed):

| Variable | Value |
|----------|-------|
| `MT_SELECTION`, `MT_WORD`, `MT_LINE`, `MT_LINK`, `MT_PATH` | what was right-clicked |
| `MT_CWD`, `MT_TITLE`, `MT_DOMAIN` | the clicked pane |
| `MT_PANE`, `MT_TAB`, `MT_WINDOW` | ids of the clicked pane, its tab and window |
| `MT_MENU`, `MT_ITEM` | the menu and item that ran |

Such a program gets the `MICROTERM_*` variables (and so `microterm ctl` access) only when the item
sets `spawn.ctl = true`; otherwise they are set empty, which also stops `ctl` from finding the
socket on its own.

`MICROTERM_UNIX_SOCKET` is only used by clients. A new GUI instance always creates its own socket,
even when started from inside a pane.

## Read by microterm

| Variable | Used for |
|----------|----------|
| `MICROTERM_UNIX_SOCKET` | which instance `microterm ctl`, `record` and a second `microterm` talk to |
| `MICROTERM_PANE`, `MICROTERM_TAB`, `MICROTERM_WINDOW` | the default target of `microterm ctl` ([Selectors & targeting](/docs/selectors-and-targeting/)) |
| `MICROTERM_CONFIG` | path of the config file |
| `XDG_RUNTIME_DIR` | socket directory on Linux (`$XDG_RUNTIME_DIR/microterm/`) |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | config and state locations |
| `VISUAL`, `EDITOR` | editor for *Open config* |
| `SHELL` | the shell, when config `shell` is empty |
