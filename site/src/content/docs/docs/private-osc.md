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
| 7720 | `progress` | named progress bars with ETA |
| 7719 | `tab` | the color of the sender's tab |
| 7718 | `badge` | a small pill badge in the pane |
| 7722 | `mark` | notes pinned to output rows (gutter and scrollbar) |
| 7723 | `key` | key bindings that act only on the sender's pane |
| 7724 | `palette` | command-palette entries for the sender's pane |
| 7721 | `fold` | collapsible output regions (display only) |
| 7726 | `link` | link rules for the program's own text conventions |

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

## `7720` progress bars

`ESC ] 7720 ; progress ; set ; <id> ; <pct> ; <b64 label> [; <eta_s>] ST` sets or replaces a bar,
`… ; progress ; done ; <id> ST` finishes it (100 % for 2 seconds, then it goes) and
`… ; progress ; clear [; <id>] ST` removes one or all. `id` is `[a-z0-9_-]{1,32}`, `pct` is `0`–`100`
or `-` for an indeterminate bar, `eta_s` is whole seconds (0–359999). A pane holds at most 6 bars,
labels are up to 48 characters, and 10 `set` per second are applied (when a program sends more, the
newest value of each bar wins). A bar nobody updates for 10 minutes is dropped.

```sh
bar() { printf '\e]7720;progress;set;%s;%s;%s;%s\a' "$1" "$2" "$(printf %s "$3" | base64)" "$4"; }
for p in 0 25 50 75 100; do
  bar fetch "$p" Fetch $(( (100 - p) / 2 ))
  bar build "$(( p / 2 ))" Build $(( 100 - p ))
  bar test - Test
  sleep 1
done
for id in fetch build test; do printf '\e]7720;progress;done;%s\a' "$id"; done
```

The tab shows **one** line on its bottom edge: the mean of the determinate bars of its panes, or an
indeterminate bar when none is determinate. A live `ctl pane status` or OSC 9;4 value of the tab
wins, and the bars are listed under it. The tab tooltip has a row per bar (`Build 42 % · 1 m 20 s
left`). The bars of the focused pane also show as a thin strip above the status bar, each with its
label and percent (`…` while indeterminate), and the pulse of an indeterminate bar stops when the
system asks for reduced motion. Bars never raise a notification or an attention marker.

## `7719` declared tab color

`ESC ] 7719 ; tab ; color ; <value> ST` colors the tab of the pane that sent it. `value` is one of
the nine tab color names, a name from `[tab_bar.palette]`, or `#rrggbb`; an empty value clears.

```sh
printf '\e]7719;tab;color;orange\a'     # a prod host's login script
printf '\e]7719;tab;color;\a'            # back to the usual color
```

The color is *declared*: it never overrides a color set by hand or by a trigger, and it wins over a
color from the config or a color rule (see [Tabs](/docs/tabs-and-workspaces/)). One color per pane;
the tab shows the focused pane's, else the first pane that declared one. The tab tooltip says
`Color: orange (declared by pane 2)`, and the color is lightness-clamped so the tab stays legible.
It is removed when the program exits, on a terminal reset, when the pane closes and when the
switch or the `tab` gate goes off.

Both sequences work in SSH and serial panes, and the `microterm-mux` daemon replays them after a
reconnect.

## `7718` pane badge

`ESC ] 7718 ; badge ; set ; <b64 text> [; <token>] ST` shows a small pill badge (a dot in the token color and bold text) in the
bottom-right of the pane that sent it; `badge ; clear ST` removes it. `token` is one of `muted`
(default), `accent`, `success`, `warning` or `error`: theme tokens only, an arbitrary color is
malformed. The text is at most 24 characters on one line; 2 changes per second.

```sh
ssh() { command ssh "$@"; printf '\e]7718;badge;clear\a'; }   # wrapper: badge only while connected
printf '\e]7718;badge;set;%s;error\a' "$(printf PROD | base64)"
```

The badge is drawn by the interface, above the terminal: a little larger than the terminal font, never taller than
22 % of the pane, click-through, hidden in panes narrower than 24 columns and still shown on the
alternate screen. It is not part of the text grid, so selection, copy, `get-text`, GIF and
screenshot exports never contain it. The pane's right-click menu names it as set by a program. It
is removed when the program exits, on a terminal reset, when the pane closes and when the switch or
the `badge` gate goes off.

## `7722` gutter marks

`ESC ] 7722 ; mark ; add ; <kind> ; <b64 note> ST` pins a note to the **row the cursor is on**;
`mark ; clear ST` removes the pane's marks. `kind` is `info`, `warn` or `error`; the note is at
most 120 characters. Up to 500 marks per pane (the oldest is dropped), 20 per second, primary
screen only (on the alternate screen the sequence is malformed).

```sh
# make wrapper: mark every line that says "error:"
make "$@" 2>&1 | while IFS= read -r line; do
  case $line in *error:*) printf '\e]7722;mark;add;error;%s\a' "$(printf %s "$line" | cut -c1-120 | base64 | tr -d '\n')";; esac
  printf '%s\n' "$line"   # the mark goes first: it pins to the row the cursor is on
done
```

A mark shows as an edge mark in the gutter and a tick on the scrollbar, in the color of its kind.
The tick sits where the row is in the whole scrollback, so with a long history it is not level with
the edge mark. It is anchored to the line's stable id, so it stays on its row when the window is
resized and goes away only when the line leaves the scrollback. Program marks join the same list as
[trigger markers](/docs/triggers/): `NextMarker`, `PreviousMarker` and `ClearMarkers` visit and
clear both, and the palette's Markers section shows `program (7722)` or `rule <name>` as the
origin. They work with `triggers.enabled = false`. While the pane is being recorded, each new mark
also adds a marker to the recording. `ED 3` (clear scrollback), a terminal reset, pane exit, the
switch and the `mark` gate clear them.

Both sequences work in SSH and serial panes, and `microterm-mux` replays them after a reconnect
(marks keep their rows).

## `7723` pane key bindings and `7724` palette entries

A program can register chords and command-palette entries that act **only on its own pane**.

```sh
mt-key add --id t ctrl+alt+t -- make test          # Ctrl+Alt+T types "make test" + Enter
mt-key add --id n --ctl ctrl+alt+n -- pane notify   # run `microterm ctl pane notify`
mt-key clear [ID]
mt-palette add "Run tests" -- make test            # a "From pane" palette entry
mt-palette clear [ID]
```

`mt-key` and `mt-palette` are defined by `microterm shell-integration` (bash, zsh, fish) and write
`ESC ] 7723 ; key ; add ; <id> ; <chord> ; <send|ctl> ; <b64 payload> ST` and
`ESC ] 7724 ; palette ; add ; <id> ; <b64 label> ; <send|ctl> ; <b64 payload> ST`;
`clear` removes one entry or all of the pane's. Up to 16 of each per pane, payloads up to 1 KiB,
labels up to 256 bytes, ids `[a-z0-9_-]{1,32}`. Keys accept 5 commands per second and palette
entries 10, in one window shared with `7717` menu items.

- **A chord needs `ctrl`, `alt` or `cmd`** (or be a function key such as `f5`).
- **They never override you.** A chord you bound in `[[keys]]`, a built-in default, or a reserved chord
  (anything that quits, closes, copies, pastes, opens the palette, leaves copy mode or toggles the
  master switch, such as `cmd+q`) is refused and counted as malformed. Pane bindings are looked at last,
  and only while that pane has focus, so the same chord does nothing in another pane.
- **Palette entries** appear at the top of the command palette with the chip `pane` while that pane has
  focus. They are never added to the right-click menu (that is `7717`).
- **Safety.** They run under the same rules as [shell-registered menu items](/docs/context-menus/#items-registered-by-the-shell):
  `send` text is sanitised, must be one line and is confirmed when it looks dangerous or the pane is
  remote; `ctl` can only act on the pane itself; `spawn` is not available. In SSH and serial panes both are
  ignored until `[menu_security] allow_remote_items = true`. Anything that can print to the terminal can
  register them, so treat them like the menu items.

They are removed when the program exits, on a terminal reset, with `mt-key clear`, when the pane closes
and when the switch or the `key` / `palette` gate goes off. `microterm-mux` replays them after a
reconnect. Agents (MCP) have no tool for them.

## `7721` collapsible output

A program can mark a region of output that you can fold into one summary row.

```sh
mt-fold begin build "Build"      # ESC ] 7721 ; fold ; begin ; <id> ; <b64 title> ST
make
mt-fold end build                # ESC ] 7721 ; fold ; end ; <id> ST
```

A CI wrapper folds each step by calling these around the step's command. Click a `▸ title · N lines`
row to expand it again. **Toggle fold at cursor** collapses or expands the region under the pointer (or around the cursor when the pointer is elsewhere); with `fold_commands = true` the right-click menu also has **Fold/unfold output** for the clicked row. The palette also has **Fold all output regions** and
**Unfold all output regions** from the palette or context menu (no default keys).

- **Display only.** Folding changes what is drawn and nothing else: the grid, scrollback, selection
  text, `get-text`, search, recordings and exports keep every line. Select across a folded region
  and copy: the hidden text is copied. Search expands the region it finds a match in.
- **Limits per pane.** 200 regions (oldest dropped), nesting depth 4, one open region per id,
  titles up to 80 characters, 20 `begin`/`end` per second. Primary screen only. A region still open
  is drawn up to the last row and never hides the row the program is writing to.
- **Malformed** (counted, ignored): `end` without `begin`, closing an outer region before the inner
  one, a reused open id, regions whose rows scrolled out of the history.
- Folds follow the stable line ids, so they survive resizing and scrollback trimming; `ED 3`
  (clear scrollback) and a terminal reset clear them. Collapsed state belongs to the view: after a
  reconnect every region arrives expanded. Folds work in SSH panes (they are display only).
- `[private_osc] fold_commands = true` also makes the output of each finished shell command
  (needs [shell integration](/docs/shell-integration/)) a fold unit named after the command.

## `7726` semantic links

A program can register link rules for text that follows its own conventions.

```sh
mt-link add issue '#(\d+)' 'https://example.com/issues/$1'
echo "fixed in #4821"            # #4821 is now a link
mt-link clear [issue]
```

`ESC ] 7726 ; link ; add ; <id> ; <b64 regex> ; <b64 template> ST` and
`ESC ] 7726 ; link ; clear ; [<id>] ST`. Up to 8 rules per pane, regex up to 256 bytes, template up
to 512 bytes, 5 commands per second. Rules use the same linear-time regex engine and complexity cap
as [`[[hyperlink_rules]]`](/docs/hyperlinks/): lookaround, backreferences and expensive patterns are
refused as malformed.

- Program rules rank **below** your own rules and OSC 8 links, apply to the visible rows only and never
  rewrite the text.
- The resolved URL must use a scheme in `hyperlinks.schemes`, and the tooltip always shows the real
  target before you open it. A rule can never open a scheme you did not allow.
- In SSH and serial panes the rules are ignored unless `[menu_security] allow_remote_items = true`.
- They are removed on exit, terminal reset, `mt-link clear` and when the switch or the `link` gate
  goes off.

## Checking the state

```sh
microterm ctl private-osc status          # layers, per-feature switches, per-pane counters
microterm ctl private-osc status --json
microterm ctl events --type private_osc.changed
```

The counters are `applied`, `malformed`, `dropped` (rate limit), `swallowed_off` and
`swallowed_disabled`.
