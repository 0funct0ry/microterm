---
title: Private OSC
description: microterm's own escape sequences in the range 7700-7799, the master switch that guards them, and the two sequences available today.
---

Programs talk to a terminal with escape sequences. microterm reserves the **OSC numbers
7700–7799** for its own extensions (`ESC ] <number> ; … BEL`). Other terminals use 7, 8, 9, 99,
133, 633, 777, 1337 and 5522, so nothing in this block collides with them.

Terminal output is untrusted, so the whole range is **off by default** and sits behind one
**master switch**.

## The master switch

```toml
# microterm.toml
[private_osc]
enabled = false          # the master switch
rate_per_second = 20     # stateful sequences per pane per second (2-100)

[private_osc.features]   # one switch per sequence, all true once enabled = true
# menu = true  caps = true  status = true  progress = true  tab = true  badge = true
# mark = true  key = true   palette = true  fold = true      link = true
```

- While `enabled = false`, **every** sequence of 7700–7799 is dead, including `7717`
  ([shell-registered menu items](/docs/context-menus/)). It is swallowed so it never prints
  garbage, nothing is parsed, nothing is stored and nothing is ever sent back.
- Turning it off (config edit, the **Toggle private OSC** palette action, or
  `microterm ctl private-osc disable`) clears everything programs registered, in every pane,
  and turning it on again does **not** bring it back: programs have to register again.
- **Nothing a program prints can turn it on**, and neither can `ctl`, the socket, the MCP server
  or a remote host. `ctl private-osc` has `status` and `disable` and deliberately no `enable`.
  You turn it on in the config file or with the palette action, at the keyboard.
- **Toggle private OSC** overrides the config for the rest of the session in both directions.
  While it holds the switch off, a lock icon shows in the status bar.
- If the config cannot be read, the last valid values apply; on first start with an invalid file
  the switch is off.

A dead sequence is indistinguishable from an unsupported one: the only visible difference is
that `7727` gets no answer, so programs must use a timeout.

## Wire format

`ESC ] <number> ; <name> ; <verb> ; <fields…> ST`, where ST is `ESC \` or BEL and `<name>` is the
sequence's name (a mismatch is malformed). Free-text fields are standard **base64**; identifiers
are `[a-z0-9_-]`. The whole string is at most 8 KiB. Malformed input is ignored, counted and never
applied partially. Text that reaches the interface is stripped of control and bidi characters and
shown as plain text only.

| OSC | Name | What it does |
|---|---|---|
| 7717 | `menu` | shell-registered context-menu items |
| 7727 | `caps` | asks what this terminal supports |
| 7725 | `status` | named status segments in the status bar |

Everything else in 7700–7799 is reserved and ignored.

## `7727` capability handshake

```sh
printf '\e]7727;caps;query\a'
```

microterm answers on the PTY with `ESC ] 7727 ; caps ; <base64 JSON> ST` (the same terminator),
at most twice a second per pane and only while the switch and the `caps` gate are on. It is the
**only** sequence that ever writes anything back. The JSON lists every sequence name with its
protocol version, or `0` when it is off or not built yet, plus a few limits. It never names which
layer disabled a feature and never includes paths, user names, pane ids or config values.

```sh
# read the reply with a timeout (a terminal without support never answers)
stty -echo -icanon min 0 time 20
printf '\e]7727;caps;query\a'; reply=$(dd bs=1 count=2048 2>/dev/null | tr -d '\033\007')
stty sane
```

## `7725` status segments

`ESC ] 7725 ; status ; set ; <slot> ; <b64 text> [; <level> [; <b64 tooltip>]] ST` sets a
segment, `… ; status ; clear [; <slot>] ST` removes one or all. `level` is `info`, `ok`, `warn`
or `error`. A pane holds at most 8 slots (`[a-z0-9_-]{1,32}`), text up to 48 characters, a tooltip
up to 256 bytes, and 5 `set`/`clear` per second.

```sh
seg() { printf '\e]7725;status;set;%s;%s;%s\a' "$1" "$(printf %s "$2" | base64)" "${3:-info}"; }
seg git "main ✔" ok
seg tests "2 failing" error
printf '\e]7725;status;clear;tests\a'
```

The segments of the **focused pane** appear as chips on the right of the status bar in slot-name
order; hovering one says which pane set it. A segment never raises a notification. Segments are
removed when the program exits, on a terminal reset, when the pane closes and when the switch or
the `status` gate goes off. They survive an SSH reconnect because the `microterm-mux` daemon
replays them.

## Checking the state

```sh
microterm ctl private-osc status          # layers, per-feature switches, per-pane counters
microterm ctl private-osc status --json
microterm ctl events --type private_osc.changed
```

The counters are `applied`, `malformed`, `dropped` (rate limit), `swallowed_off` and
`swallowed_disabled`.
