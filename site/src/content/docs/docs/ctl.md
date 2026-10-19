---
title: microterm ctl
description: Every microterm ctl command, organised by resource, with flags and examples.
---

`microterm ctl` scripts a running instance. It is organised by **resource**: `workspace`,
`window`, `tab` and `pane`, plus a few top-level commands. All commands accept `--json` and talk
to the instance described in the [CLI overview](/docs/cli-overview/). How a command names the
item it acts on is covered in [Selectors & targeting](/docs/selectors-and-targeting/).

```text
microterm ctl list
microterm ctl workspace list | create NAME | switch NAME | rename OLD NEW | delete NAME [--force]
microterm ctl window    list | new | focus | close
microterm ctl tab       list | new | focus | rename | close
microterm ctl pane      list | get | send-text | get-text | run | wait | notify | status | close
microterm ctl events | record | replay | reload-config
```

Every resource has the same verbs where they make sense: `list`, `get`, `new`, `focus`, `rename`
and `close` (`delete` for workspaces). Verbs that only exist for panes (`send-text`, `get-text`)
live under `pane`.

## list

```sh
microterm ctl list [--json]
```

Prints workspaces, windows, tabs and panes as a tree. `--json` returns
`{"workspaces":[{"id","name","active","windows":[{"id","title","active","focused","tabs":[{"id","title","active","focused","panes":[<pane>]}]}]}]}`
where `<pane>` has the shape of `pane get --json`, below.

## workspace

```sh
microterm ctl workspace list
microterm ctl workspace create NAME
microterm ctl workspace switch NAME
microterm ctl workspace rename OLD NEW
microterm ctl workspace delete NAME [--force]
```

`list --json` returns `[{"id","name","active","windows":[window ids]}]`. `delete` closes every
window, tab and pane in the workspace. The `default` workspace and the active workspace cannot be
deleted (exit 1). Like every close, `delete` honours [`confirm_close`](#closing-and-confirm_close).

## window

```sh
microterm ctl window list [--match EXPR]
microterm ctl window new [--workspace NAME] [--cwd DIR] [-- CMD ARGS…]
microterm ctl window focus [--window ID | --match EXPR]
microterm ctl window close [--window ID | --match EXPR] [--force] [--all]
```

`new` opens a window with one tab and prints `tab=5 pane=6 window=2`, which is safe to `eval`.
`--workspace` opens it in that workspace, creating it if needed and switching to it. `focus`
brings the window to the front (switching to its workspace first). `list --json` returns
`[{"id","title","workspace","active","focused","tabs":[tab ids]}]`; `active` means the window
belongs to the active workspace.

## tab

```sh
microterm ctl tab list [--window ID] [--match EXPR]
microterm ctl tab new [--window ID] [--workspace NAME] [--domain NAME] [--cwd DIR]
                      [--title T] [-- CMD ARGS…]
microterm ctl tab focus [--tab ID | --match EXPR | --index N | --relative N] [--window ID]
microterm ctl tab rename [--tab ID | --match EXPR] TITLE
microterm ctl tab close [--tab ID | --match EXPR] [--force] [--all]
```

`new` prints `tab=… pane=… window=…`. Without `--window` the tab goes into the focused window of
the target workspace and becomes its active tab. `--workspace` switches to that workspace
(creating it when it does not exist); the first tab opened in an empty workspace becomes that
workspace's first tab. `--domain` takes `local`, the name of an [SSH domain](/docs/ssh-domains/),
or `ssh:[user@]host[:port]` for a temporary one.

`focus` makes a tab active: a target, `--index N` (zero-based, within the window) or
`--relative N` (from the active tab, wrapping around). `rename` sets the title; an empty title
restores the automatic one. `list --json` returns
`[{"id","title","window","workspace","domain","active","focused","panes":[pane ids]}]`.

## pane

```sh
microterm ctl pane list [--window ID] [--tab ID] [--match EXPR]
microterm ctl pane get [--pane ID | --match EXPR]
microterm ctl pane send-text [--pane ID | --match EXPR] [--all] [--no-paste] [TEXT]
microterm ctl pane get-text [--pane ID | --match EXPR] [--start-line N] [--end-line N] [--escapes]
                            [--last-command [N]]
microterm ctl pane run [--in TARGET] [--keep] [--detach] [--timeout DUR] [--propagate-exit] -- CMD…
microterm ctl pane wait [TARGET] [--match REGEX] [--idle DUR] [--exit] [--command-finished]
                        [--timeout DUR] [--new-only]
microterm ctl pane notify [TARGET] [--title T] [--level info|attention|error] [--desktop] [MESSAGE]
microterm ctl pane status set [TARGET] [--state S] [--progress N] [--ttl DUR] [TEXT]
microterm ctl pane status clear|get [TARGET]
microterm ctl pane close [--pane ID | --match EXPR] [--force] [--all]
```

`list` and `get` return the pane shape:

```json
{"id":12,"tab":4,"window":2,"workspace":"api","domain":"local","title":"cargo watch",
 "cwd":"/Users/me/src/api","process":"cargo","size":{"cols":120,"rows":34},
 "focused":false,"exited":false,"exit_code":null,
 "status":{"state":"working","text":"running tests","progress":40,"updated_at":"2026-10-18T09:14:03.000Z"}}
```

`status` is `null` until something sets one.

`process` is the foreground process when it is not the shell (`null` at an idle prompt, and
always `null` in SSH panes). A pane is removed from the mux when its process exits, so `exited`
is `false` and `exit_code` `null` for every pane except one kept open by `pane run --keep`.

`send-text` sends TEXT, or stdin when TEXT is omitted. By default the text is **pasted**:
wrapped in bracketed-paste markers when the program asked for them (so editors and shells do not
run it line by line), and with newlines turned into carriage returns otherwise. `--no-paste`
types the bytes exactly as given, which is what you want for control characters (`$'\x03'`).
`--all` sends to every pane the selector matches.

`get-text` prints the pane's text. Lines are numbered from the top visible row (`0`); negative
numbers reach into scrollback (`-1` is the row just above the screen). Soft-wrapped rows are
joined and trailing blank lines dropped. `--escapes` re-encodes colours and styles as SGR
sequences. With `--json` the result is `{"text": "…"}` (`"truncated": true` when `[ctl].capture_max_bytes`
cut the start). `--last-command [N]` prints the output of the N-th most recent finished command
instead; see [Run & wait](/docs/run-and-wait/).

```sh
microterm ctl pane get-text --pane 12 --start-line -200 --end-line -1
microterm ctl pane get-text --match 'title:build*' --escapes > styled.txt
```

`run` runs a command and returns its output and status, `wait` waits for output, idleness, an
exit or a finished command. They are described in [Run & wait](/docs/run-and-wait/), and the
shell setup they need for `--in`, `--command-finished` and `--last-command` in
[Shell integration](/docs/shell-integration/).

`notify`, `status` and the top-level `events` command are described in
[Notifications & status](/docs/notifications-and-status/).

```sh
microterm ctl pane run --detach --title dev -- npm run dev
microterm ctl pane wait --match title:dev --match 'ready on' --timeout 120s
microterm ctl pane run --json -- cargo test        # {"exit_code":…,"output":"…",…}
```

## Closing and confirm_close

`window close`, `tab close`, `pane close` and `workspace delete` follow the
[`confirm_close`](/docs/config-file/) policy but **never open a dialog**, so a script cannot
hang:

- If the policy would ask (`always`, or `process` with a program running in the pane) and
  `--force` is not given, the command exits **8** and closes nothing.
- With `--force` it closes immediately.
- Closing a tab's last pane closes the tab, and closing a window's last tab closes the window.

A command running inside a pane can close its own tab: `microterm ctl tab close --force`.

## record

```sh
microterm ctl record [--pane ID] -o FILE.cast [--duration SECS] [--input]
```

Records a pane until its process exits, `--duration` passes or Ctrl-C. See
[Recording & replay](/docs/recording-and-replay/).

## replay

```sh
microterm ctl replay FILE.cast [--speed 1.0] [--new-window]
```

Plays the file into a new tab (or window) and prints `tab=… pane=… window=…`.

## reload-config

```sh
microterm ctl reload-config
```

Re-reads the config file immediately, as if it had just changed on disk.
