---
title: microterm cli
description: Every microterm cli subcommand, with flags and examples.
---

All subcommands accept `--json` and target the instance described in the
[CLI overview](/docs/cli-overview/). `--pane` defaults to `$MICROTERM_PANE`.

## list

```sh
microterm cli list [--json]
```

Prints workspaces, windows, tabs and panes. `--json` returns
`{"workspaces":[{"id","name","active","windows":[{"id","title","tabs":[{"id","title","active","panes":[{"id","title","cwd","domain"}]}]}]}]}`.

## spawn

```sh
microterm cli spawn [--window ID | --new-window] [--workspace NAME] [--domain NAME]
                    [--cwd DIR] [--title T] [-- CMD ARGS…]
```

Opens a tab and prints the new **pane id**. Without `--window` the tab goes into the first window
of the target workspace. `--workspace` switches to that workspace (creating it when it does not
exist); the first tab spawned into an empty workspace becomes that workspace's first tab.
`--domain` takes `local`, the name of an [SSH domain](/docs/ssh-domains/), or
`ssh:[user@]host[:port]` for a temporary one. The new tab becomes the active tab.

## send-text

```sh
microterm cli send-text [--pane ID] [--no-paste] [TEXT]
```

Sends TEXT to the pane, or stdin when TEXT is omitted. By default the text is **pasted**: wrapped
in bracketed-paste markers when the program asked for them (so editors and shells do not run it
line by line), and with newlines turned into carriage returns otherwise. `--no-paste` types the
bytes exactly as given, which is what you want for control characters (`$'\x03'`).

## get-text

```sh
microterm cli get-text [--pane ID] [--start-line N] [--end-line N] [--escapes]
```

Prints rows of the pane. Line `0` is the top visible row; negative numbers reach into scrollback
(`-1` is the row just above the screen). The default is the visible screen. `--end-line` is
inclusive. Soft-wrapped rows are joined, trailing blank lines are dropped. `--escapes` keeps the
styling, re-encoded as SGR sequences.

```sh
microterm cli get-text --pane 3 --start-line -200      # the last 200 lines of scrollback + screen
```

## activate-tab, set-tab-title, kill-pane

```sh
microterm cli activate-tab (--tab-id ID | --tab-index N | --tab-relative N)
microterm cli set-tab-title [--tab-id ID] TITLE     # empty TITLE removes the override
microterm cli kill-pane [--pane ID]
```

`--tab-relative` wraps around. Without `--tab-id`, `set-tab-title` renames the active tab.
Killing the last pane of a window closes the window.

## workspace

```sh
microterm cli workspace list
microterm cli workspace switch NAME
microterm cli workspace create NAME
microterm cli workspace rename OLD NEW
```

`default` always exists and cannot be renamed.

## record and replay

```sh
microterm cli record [--pane ID] -o FILE.cast [--duration SECS] [--input]
microterm cli replay FILE.cast [--speed 1.0] [--new-window]
```

See [Recording & replay](/docs/recording-and-replay/).

## reload-config

```sh
microterm cli reload-config
```

Re-reads the config file, as the *Reload config* action does.

## Events

Scripts that need to react to changes can speak the protocol directly. After a `subscribe`
request the connection streams one JSON line per mux event (`pane_added`, `pane_removed`,
`pane_output`, `title_changed`, `tab_added`, `tab_removed`, `tab_moved`, `window_added`,
`window_removed`, `workspace_switched`) without an `id`:

```sh
printf '{"v":1,"id":1,"method":"subscribe"}\n' | socat - UNIX-CONNECT:"$MICROTERM_UNIX_SOCKET"
```

Requests look like `{"v":1,"id":7,"method":"pane.send_text","params":{"pane":12,"text":"ls\n","paste":true}}`
and are answered with `{"v":1,"id":7,"result":{}}` or
`{"v":1,"id":7,"error":{"code":"not_found","message":"…"}}`. Error codes are `bad_request`,
`not_found`, `version_mismatch` and `internal`. A peer that speaks another major protocol version
gets `version_mismatch` (exit code 4 in the CLI).
