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
microterm ctl tab       list | new | focus | rename | color | close
microterm ctl pane      list | get | send-text | get-text | run | wait | notify | status | close
microterm ctl events | record | replay | reload-config | skill | mcp
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
microterm ctl tab color [--tab ID | --match EXPR] (NAME | '#rrggbb' | --clear)
microterm ctl tab close [--tab ID | --match EXPR] [--force] [--all]
```

`new` prints `tab=… pane=… window=…`. Without `--window` the tab goes into the focused window of
the target workspace and becomes its active tab. `--workspace` switches to that workspace
(creating it when it does not exist); the first tab opened in an empty workspace becomes that
workspace's first tab. `--domain` takes `local`, the name of an [SSH domain](/docs/ssh-domains/),
or `ssh:[user@]host[:port]` for a temporary one.

`color` sets the manual color of a tab (a name, `#rrggbb` or `--clear`) and prints what applies
now: `pink (manual)`, `red (declared)`, `teal (rule)` or `none`. `tab new --color NAME` opens a tab
already colored. An unknown name is a usage error that lists the valid ones. `tab list --json` and
`list --json` carry `color` and `color_source` (`manual`, `declared` or `rule`; absent when the
tab has none). `microterm ctl events --type tab.color_changed` streams `{tab, color, source}`
whenever the resolved color changes, including when a rule starts or stops matching.

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
microterm ctl pane split [TARGET] (--horizontal | --vertical | --direction D) [--percent N] [--cwd DIR] [-- CMD…]
microterm ctl pane focus [TARGET] [--direction left|right|up|down]
microterm ctl pane zoom [TARGET]
microterm ctl pane resize [TARGET] --direction D [--cells N]
microterm ctl pane swap [TARGET] --direction D
microterm ctl pane rotate [TARGET]
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

## recording

```sh
microterm ctl recording list
microterm ctl recording show ID
microterm ctl recording start --pane ID [--input | --no-input] [--title T]
microterm ctl recording stop (--pane ID | ID | --all)
microterm ctl recording marker --pane ID [--label L]
microterm ctl recording rename ID TITLE
microterm ctl recording rm ID… --force
microterm ctl recording export ID --format cast|vhs [-o PATH] [--opt KEY=VALUE]… [-f]
microterm ctl recording export --list-formats
microterm ctl recording replay ID [--speed 1.0] [--new-window]
```

Works on the [recordings store](/docs/recording-and-replay/#the-recordings-store). While
`recording.enabled = false` every verb exits `10` and the store directory is not touched.

- `start`, `stop` (by pane) and `marker` are input-class verbs: they need `--pane` or
  `$MICROTERM_PANE` and never fall back to the focused pane. `start` prints the recording id; it
  follows `recording.capture_input` unless `--input` or `--no-input` is given.
- `stop` takes `--pane`, the recording id, or `--all`, and prints the saved ids.
- `list` and `show` print the metadata (`--json` gives the sidecar plus the `.cast` `path`; `list`
  also reports running recordings). An unknown id exits `3`.
- `rm` never blocks on a dialog: without `--force` it exits `8` and deletes nothing; with it,
  both files are removed for good (there is no trash).
- `replay` plays the stored `.cast` into a new tab, like `ctl replay FILE`.

## menu

```sh
microterm ctl menu list
microterm ctl menu show MENU [TARGET]
microterm ctl menu invoke MENU/ITEM [TARGET] [--dry-run] [--force]
microterm ctl menu check [--config PATH]
microterm ctl menu icons
```

Lists, shows and runs [context-menu](/docs/context-menus/) items, built-in (`viewport/zoom`,
`viewport/split_left`, `tab/close`) or configured. `invoke` is an input verb: it needs `--pane`,
`--match` or `$MICROTERM_PANE` and never falls back to the focused pane. It exits 3 for an unknown
item, 1 when the item's `when` does not hold, and 8 when the item would ask for confirmation
(pass `--force`). `--dry-run` prints the expanded argv or text and runs nothing. `check` validates
the menu config and `icons` lists the names usable as `icon = "…"`, both with no running instance.

## snippet

```sh
microterm ctl snippet list [--folder F] [--tag T] [--query Q]
microterm ctl snippet show ID
microterm ctl snippet expand (ID | --body TEXT) [TARGET] [--var LABEL=VALUE]… [--vars-file FILE|-]
microterm ctl snippet insert (ID | --body TEXT) TARGET [--var LABEL=VALUE]… [--vars-file FILE|-]
                             [--run | --no-run] [--all]
microterm ctl snippet add --title T [--id ID] [--folder F] [--tag T]… [--description D]
                          [--run] [--key K] [--replace] [BODY]
microterm ctl snippet rm ID
```

Works on [snippets](/docs/snippets/). While `snippets.enabled = false` every verb exits `10`.

- `list`, `show`, `add` and `rm` work on `snippets.toml`; `add` and `rm` write it back like the
  panel does (comments and other entries untouched) and refuse an existing id without `--replace`.
  `add` reads the body from stdin when `BODY` is omitted.
- `expand` prints what a snippet expands to for a pane (`--pane`, `--match`, `$MICROTERM_PANE`,
  else the focused pane) and changes nothing; counters do not advance.
- `insert` types the snippet into a pane. It is an input verb: it needs `--pane`, `--match` or
  `$MICROTERM_PANE` and never falls back to the focused pane. `--run` presses Enter, `--no-run`
  never does, otherwise the snippet's own `run` decides; `--all` inserts into every match.
- Prompt values come from `--var Label=value` and `--vars-file` (a JSON object `{label: value}`,
  `-` is stdin). Use `--vars-file -` for `secret` values, because argv is visible to other
  processes. A missing value exits `11` and lists every label that still needs one; an `input`
  with a `default=` does not need one. With `snippets.templates = false` the body is used as
  written and `--var` is ignored with a warning.

```sh
microterm ctl snippet insert k8s-logs --pane $MICROTERM_PANE --var Namespace=dev --var Pod=api --run
echo '{"Token":"s3cret"}' | microterm ctl snippet insert deploy --pane 3 --vars-file -
```

## reload-config

```sh
microterm ctl reload-config
```

Re-reads the config file immediately, as if it had just changed on disk.

## skill

```sh
microterm ctl skill install|uninstall|show [--agent claude|codex|antigravity|generic|all]
                                           [--scope user|project] [--dir PATH] [--print]
```

Installs agent-facing instructions for `ctl`. Needs no running instance. See
[Agents & automation](/docs/agents-and-automation/).

## mcp

```sh
microterm ctl mcp
```

Runs an MCP server on stdio that exposes `ctl` as tools. See
[Agents & automation](/docs/agents-and-automation/).

## Interactive use

`microterm sh` is a REPL over exactly these commands, with aliases, a sticky target and
completion of IDs and selectors. See [Interactive shell](/docs/sh/).
