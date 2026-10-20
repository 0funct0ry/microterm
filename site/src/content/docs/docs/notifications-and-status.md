---
title: Notifications & status
description: Let long-running processes and agents tell you what they are doing, and let scripts react to terminal events.
---

Three `microterm ctl` commands connect a running process to the human watching the window:

- `pane status` shows what a process is doing on its **tab** (dot, progress line, tooltip).
- `pane notify` raises an **attention badge**, a toast and an OS notification.
- `events` streams terminal events to a script as JSON lines.

All of them follow the usual [targeting rules](/docs/selectors-and-targeting/): inside a pane they
default to that pane through `$MICROTERM_PANE`. `status set|clear` and `notify` never fall back to
the focused pane, so a script in another terminal cannot decorate the wrong tab.

## Status

```sh
microterm ctl pane status set [TARGET] [--state idle|working|waiting|done|error] [--progress 0-100]
                              [--ttl DUR] [TEXT]
microterm ctl pane status clear [TARGET]
microterm ctl pane status get [TARGET]
```

| State | Tab dot |
|---|---|
| `working` (the default) | accent |
| `waiting` | warning: "needs a human" |
| `done` | success |
| `error` | error |
| `idle` | none |

`--progress` draws a 2 px line along the tab's bottom edge. The text appears in the tab's
tooltip and in the command palette's `#` tab list, and every status is part of `pane list --json`:

```json
"status":{"state":"working","text":"running tests","progress":40,"updated_at":"2026-10-18T09:14:03.000Z"}
```

`status` is `null` when nothing is set. The `status:` [selector](/docs/selectors-and-targeting/) key
matches the state (`idle` also matches panes with no status): `--match 'status:waiting'`.

A status clears itself when the pane's process exits or when `--ttl` (`500ms`, `30s`, `5m`)
passes, so an agent that crashes leaves no stale "working" marker. Text is stripped of control
characters and cut to `[ctl].status_max_len` characters (default 120).

```sh
microterm ctl pane status set --state working --progress 40 'running tests'
microterm ctl pane status set --state waiting --ttl 30s 'approve the deploy?'
microterm ctl pane status clear
```

## Notify

```sh
microterm ctl pane notify [TARGET] [--title T] [--level info|attention|error] [--desktop] [MESSAGE]
```

- A **badge** (a dot in `accent`, `warning` or `error`, by level) appears on the pane's tab until
  that pane is focused. A badge is only raised for a pane you are not already looking at, and a
  higher level is never downgraded by a lower one.
- A **toast** shows when the pane is not visible.
- An **OS notification** is raised when microterm is not the focused application. `--desktop`
  forces one.
- A `pane.notification` [event](#events) is emitted.

Notifications are limited to **5 per second per pane**. Extra ones are dropped; the command still
succeeds and reports the count (`"dropped": N` with `--json`, a line on stderr otherwise).

```toml
[notifications]
badge = true            # attention badge on the tab until it is focused
toast = "background"    # "background" (pane not visible) | "always" | "never"
desktop = "unfocused"   # "unfocused" (no microterm window focused) | "always" | "never"
```

These keys, like `[ctl]`, are read when the next request arrives, so a [hot reload](/docs/hot-reload/)
applies them at once. OS notifications go through D-Bus
(`org.freedesktop.Notifications`) on Linux and the system notification centre on macOS. On macOS an
unbundled development binary may show nothing; the bundled `.app` is reliable.

:::caution
Status and notification text is content chosen by whatever process sends it, and it is shown in
the app's own chrome and in OS notifications. microterm strips control characters, caps the length and
rate-limits notifications, but a hostile program can still write a convincing message, and any
process that can reach the socket can send one. `ctl` is remote control of every pane; the only
access control is the private socket directory and the same-user check.
:::

## From a program (OSC 9 and OSC 9;4)

A program does not need `microterm` to report progress or raise a notification: it can print
escape sequences, which also work over SSH.

```sh
printf '\e]9;4;1;40\e\\'        # progress 40 %: the tab shows the progress line
printf '\e]9;4;3\e\\'           # indeterminate
printf '\e]9;4;0\e\\'           # remove
printf '\e]9;build done\e\\'    # notification
```

`9;4;<state>;<progress>`: state 0 removes, 1 is normal, 2 error, 3 indeterminate, 4 paused;
progress is clamped to 0–100 and ignored for states 0 and 3. Normal and indeterminate show as
`working`, error as `error`, paused as `waiting`, with the progress value on the tab's progress
line and in `ctl pane list --json`. A status set with `ctl pane status` wins while it is live:
OSC progress neither replaces nor clears it. Progress also ends on a terminal reset and when the
pane exits.

Other ConEmu subcommands (`9;1` to `9;3`, `9;5` to `9;12`) are parsed and ignored. Any other
`OSC 9` text is a notification at level `info`. It goes through the same pipeline as `pane
notify` (badge, toast, OS notification per `[notifications]`), is sanitised, cut to
`ctl.status_max_len` and limited to 5 per second per pane, and appears in `ctl events` as
`pane.notification` with `"source": "osc9"` (`"ctl"` for `pane notify`). The text comes from
whatever runs in the pane, so set `terminal.osc_notifications = false` to ignore it.

## Events

```sh
microterm ctl events [--type T[,T…]] [--match EXPR] [--snapshot]
```

Prints one JSON object per line until interrupted:

```json
{"type":"pane.command_finished","pane":12,"exit_code":1,"ts":"2026-10-18T09:14:03.250Z"}
```

| `--type` | Fires when |
|---|---|
| `pane.added`, `pane.removed` | a pane opens or closes |
| `pane.focused`, `tab.layout_changed` | a tab's focused pane or its split layout changed |
| `pane.title_changed`, `pane.cwd_changed` | the title or working directory changed |
| `pane.exited` | the process ended (`status`) |
| `pane.command_finished` | an OSC 133 `D` mark (`exit_code`); needs [shell integration](/docs/shell-integration/) |
| `pane.status_changed` | a status was set, changed, cleared or expired |
| `pane.notification` | `pane notify` got through the rate limit (`level`, `title`, `message`) |
| `tab.added`, `tab.removed`, `tab.moved`, `window.added`, `window.removed` | the layout changed |
| `workspace.added`, `.renamed`, `.switched`, `.removed` | workspaces changed |
| `domain.state_changed` | an SSH domain connected, reconnected or dropped |

`--type` takes a comma-separated list; a trailing `.*` matches a family (`workspace.*`). With no
`--type` everything is streamed except `pane.output`, which is too noisy (use
[`pane wait`](/docs/run-and-wait/) to wait for output; name `pane.output` in `--type` if you
really want the wake-ups).

`--match` keeps the events whose pane, tab or window matches a [selector](/docs/selectors-and-targeting/):
`--match 'title:build*'`. Events about nothing in particular (workspaces, domains) never pass a
`--match`. The selector is evaluated as each event arrives, so events that announce a pane's removal
use the answer from just before it went away.

`--snapshot` first prints the whole tree as `{"type":"snapshot","tree":{…}}` (the `ctl list --json`
shape), so a consumer can start from known state without a race. The command exits **2** when the
GUI quits and `0` on Ctrl-C.

```sh
microterm ctl events --type pane.command_finished,pane.notification --json
microterm ctl events --type pane.status_changed --match 'title:agent*' | jq -r '.pane'
```
