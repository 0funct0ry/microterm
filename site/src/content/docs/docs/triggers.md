---
title: Triggers
description: Local automation rules that react to terminal output or pane events with a small, closed set of safe actions.
---

A **trigger** is a small rule: *when* a pane prints a matching line (or an event happens), run a
short list of **actions**: highlight the line, turn the tab red, send a notification, count the
hits, jump back to it later. Triggers are **off by default**.

```toml
# microterm.toml
[triggers]
enabled = true
```

With `enabled = false` the feature is inert: no line is scanned, no thread runs, no file is read
or created, and every Triggers action in the palette only shows "Triggers are disabled". Turning
it on or off takes effect on the next config reload, without a restart; turning it off removes
every highlight, marker, counter, finding, trigger-set title and color, and stops and saves any
recording a trigger started.

## Make a rule from what you see

1. Select a line of output, for example `error: build failed`.
2. **Edit ▸ Create Trigger from Selection…**, the pane's context menu, or the palette. The
   Triggers panel opens on a new rule with the selection (escaped, first line, at most 200
   characters) as its regex and a `highlight` action.
3. Give it a name, add `tab color`, `notify` and `findings` with **Add action**, and **Save**.
4. Run the command again. The line highlights, the tab turns red, you get a notification, and a
   row appears under **Findings**; click it to scroll back to the line, which flashes for
   1.2 s (the whole line, or just the match: the **Mark whole line** switch under Findings).

The panel is the right-hand side panel (one panel at a time, with Snippets and Recordings).
**View ▸ Triggers** or `ToggleTriggersPanel` opens it. It never opens by itself.

- **Rules** lists each rule with its switch, a summary (`text · /error/ · 3 actions`), how many
  times it fired, when it last fired and a red dot with the reason when an action failed or was
  skipped. The footer has **Pause all**, the count of lines dropped under load and, while any
  capture action is on and allowed, "Captures may contain secrets".
- **The editor** has the name, an *Applies to* selector with a live "matches N panes", a Text or
  Event switch, the regex with its error and a live "N matches in the last 200 lines" preview
  (the matches are also drawn in the pane while you type), the ordered actions, the cooldown and
  the buttons **Save**, **Cancel**, **Test**, **Duplicate** and **Delete**.
- **Test** is a dry run: it evaluates the rule against the last 200 lines of the focused pane and
  lists what *would* run, with the text already expanded. It runs nothing.
- **Findings** lists what `findings` actions collected, merging identical consecutive entries into
  `×N`. An entry whose line left the scrollback stays, marked "scrolled out".

## Where rules live

Rules are stored in `triggers.toml` next to `microterm.toml`. The panel writes only the rule you
edit (comments and untouched rules stay byte-identical), and hand edits reload within about
150 ms. A mistake is reported at `triggers.toml:LINE:COL` in a banner at the top of the panel and
the last valid rules stay in effect.

```toml
version = 1

[[trigger]]
id = "t_3f9a1c"                    # generated, stable, never reused
name = "Build failed"
panes = "domain:local"             # a selector over pane facts; default "*" (every pane)
scope = "output"                   # "output" (default: skip the prompt and typed input) | "any"
cooldown_ms = 1000                 # minimum gap per rule and pane, 0-60000
[trigger.when]
kind = "text"
regex = 'error(\[E\d+\])?: (?P<msg>.+)'
case_insensitive = false
[[trigger.actions]]
kind = "highlight"
scope = "line"
color = "red"
[[trigger.actions]]
kind = "notify"
level = "error"
message = "{msg}"

[[trigger]]
id = "t_77be02"
name = "Background command failed"
panes = "!focused:true"
[trigger.when]
kind = "event"
event = "command_finished"         # command_finished | cwd_changed | pane_exited | title_changed | bell
exit = "failure"                   # any | success | failure | <integer>
[[trigger.actions]]
kind = "notify"
message = "{title} exited with {exit}"
desktop = true
```

Limits: 64 rules (`triggers.max_rules`), 4 actions per rule, a regex of at most 1024 characters
(compiled with the Rust `regex` crate: matching is linear time, so there is no catastrophic
backtracking, but lookahead, lookbehind and backreferences are not supported and the error says
so), names of 60 and templates of 256 characters. A rule may not have two `title`, `tab_color`,
`status` or `counter` actions.

## Text rules

A text rule matches the **rendered** text of a logical line: wrapped rows are joined and colors
are gone, as you see it. A line is scanned when the cursor leaves it. A line still being typed
(`Password:`, `Continue? [y/N]`) is scanned **once** after `scan_partial_after_ms` (250) of
output silence, so a rule can react to a prompt that has no newline. These are never scanned: the
alternate screen (`vim`, `less`, unless `scan_alt_screen = true`), bytes replayed after an SSH
reconnect or attach (a reconnect never re-fires old matches), and the part of a line beyond
`max_line_chars` (4096).

**Output or everything.** A text rule has `scope = "output"` (the default) or `scope = "any"`.
With `output`, lines of the shell prompt and of the command you are typing (including a dimmed
autosuggestion) are skipped, so typing `echo error` does not fire a rule that looks for
`error`; only what commands print counts. With [shell integration](/docs/shell-integration/)
(OSC 133) the whole prompt is skipped. Without it, microterm skips every line you typed on (keys,
text or a paste), so the command line is still skipped but a prompt line you never typed on is
not. A prompt a *program* prints while it runs (`Continue? [y/N]`, `rm -i`) is output until you
answer it. Use `scope = "any"` to match everything. The editor's **Output only** switch, its live
preview and **Test** follow the same setting; event rules have no `scope`.

Several rules share one pass over each line. Lines are read from the screen about 2 ms after
they finish by a small thread, and scanned on a worker thread that is never on the PTY or the
UI thread; when the worker falls behind the oldest lines are dropped and counted (**dropped
lines** in the panel and in `ctl trigger list`).

## Templates

`title`, `message`, `label`, `text` and `template` fields expand `{rule}`, `{pane}`, `{title}`,
`{cwd}`, `{domain}`, `{line}` (the matched line), `{match}`, numbered captures `{1}`…`{9}` and named
captures `{name}`; event rules add `{event}` and `{exit}`. A placeholder the rule cannot produce is
an error when the file loads. Every substituted value is sanitised first (control characters and
bidirectional overrides removed, whitespace collapsed, at most 120 characters). Write a literal
`{` as `{{`. Expansion is plain substitution: no functions, no environment, no processes.

## Actions

| `kind` | What it does | Limits |
|---|---|---|
| `highlight` | Tints the matched text or the whole line (`scope`), or underlines it. Display only: the text, copy output and the PTY are untouched, and the tint never changes the glyph color. `color` is `accent`, `warning`, `error`, `success`, `info`, one of the tab color names or `#rrggbb`. Optional `ttl_s`. | 2000 per pane (`highlights_max`); not drawn on the alternate screen |
| `notify` | The same path as `ctl pane notify`: tab badge until the pane is focused, toast and OS notification per `[notifications]`. | 5 per second per pane |
| `status` | The same path as `ctl pane status set`: dot, progress line, tooltip. `progress` is 0-100 or a template. Optional `ttl_s`. | |
| `tab_color` | Sets a tab color. Precedence: manual > trigger > declared > rules. The tooltip says `Color: red (trigger: Build failed)`. Optional `ttl_s`, `clear_on_focus`; it also ends when the pane exits. | one per tab |
| `bell` | Rings the pane bell with `[bell].mode` (visual, audible or none). | 1 per second per pane |
| `counter` | Counts firings per rule and pane, exactly: a rule with only highlight, marker and counter actions is not rate limited (set `cooldown_ms = 0` to count every match) and shows a pill (`12`, `99+`) after the tab title. `reset_on` is `focus` (default), `command_start` or `never`. | stops at 9999 |
| `title` | Gives the tab a title (60 characters). A rename you type wins, and ends it; it wins over the program's own title. | |
| `marker` | Marks the matched line (programs can add their own marks to the same list with [`7722`](/docs/private-osc/)): a tick on the scrollbar, a 3 px mark at the left edge, `NextMarker`/`PreviousMarker` to jump, a "Markers" palette section (`%`). Written to a recording of the pane too. | 1000 per pane |
| `findings` | Adds an entry to the Findings list (and a marker). | 500 per window |
| `recording` | `start` records the pane through the recorder **without input** (always) and marks the sidecar `origin.trigger = "trigger:<id>"`; `stop` stops only recordings that a trigger started. Needs `recording.enabled`. Optional `stop_after_s`. | one per pane |
| `clipboard` | Copies the match (or a capture) to the system clipboard as one sanitised line of at most 4 KiB, and **always** shows a toast saying so. Never the PRIMARY selection. | gate `allow_clipboard`; 1 per 5 s per rule |
| `snapshot` | Writes the matched line plus `before` and `after` lines to a text file in the captures directory. Secret-looking text is replaced by `‹redacted›` (best effort). | gate `allow_capture`; 1 per 2 s per rule |
| `screenshot` | Writes a PNG of the pane's visible grid, drawn by the same software rasteriser as GIF export (not a window capture, so it needs no OS permission and cannot be redacted). | gate `allow_capture`; 1 per 10 s per rule, at most 4096 px and 5 MB |

Actions run in the order written; one that fails or is skipped never stops the others.

## Two more switches for actions that leave the window

`clipboard`, `snapshot` and `screenshot` each sit behind a **second gate** that can only be set in
`microterm.toml`, never from the panel or `triggers.toml`:

```toml
[triggers]
enabled = true
allow_clipboard = true   # the `clipboard` action
allow_capture = true     # the `snapshot` and `screenshot` actions
```

A rule that uses one while its gate is closed loads and is kept; the action is skipped (and
counted) and the editor shows it greyed with the hint.

Captures are written to `triggers.capture_dir` (default `~/.local/share/microterm/captures`) as
`<rule-id>-<UTC date>-<time>-<n>.txt|png`, always named by microterm, never from terminal text.
The directory is created `0700` and files `0600` without following symlinks or overwriting; a
symlinked or foreign-owned directory is refused. Per-file, file-count and total-size caps apply,
and rotation deletes only microterm's own files, oldest first. The files are **not encrypted**.

## Selectors

`panes` is the [selector](/docs/selectors-and-targeting/) syntax of `ctl`, matched against the
pane's *current* facts each time a rule fires: `domain:local`, `cmd:ssh*`, `!focused:true`,
`title:build*,cwd:~/work/*`. The foreground-process filter is just `cmd:`.

## ctl

```sh
microterm ctl trigger list [--json]      # id, name, on, kind, panes, fired, suppressed, last error
microterm ctl trigger get t_3f9a1c
microterm ctl trigger enable|disable t_3f9a1c   # this session only
microterm ctl trigger pause | resume            # this session only (the footer's Pause all)
microterm ctl trigger test t_3f9a1c --in $MICROTERM_PANE   # dry run
microterm ctl events --type trigger.fired
```

While the feature is off every verb exits 10; a rule that does not exist exits 3. **There is no
verb that creates, edits or deletes a rule**, and the MCP server exposes nothing about triggers:
rules are written in the panel or the file, so a process that can drive `ctl` cannot install
automation. `enable`, `disable`, `pause` and `resume` never write `triggers.toml`, so a script
cannot leave a rule permanently off. `ctl events` shows `pane.bell` and `trigger.fired`, and
notifications from triggers carry `source: "trigger"`.

## Actions and keys

`ToggleTriggersPanel`, `FocusTriggersPanel`, `NewTrigger`, `CreateTriggerFromSelection`,
`OpenTriggersFile`, `ToggleTriggersPaused`, `NextMarker`, `PreviousMarker` and `ClearMarkers`
have **no default keys**; bind the ones you want in `[[keys]]`. All are in the command palette
while the feature is on.

## Security, in plain language

Anything a program prints can fire a rule, so everything a rule can do has to be safe at a time
and rate an attacker chooses.

| Risk | What stops it |
|---|---|
| Hostile output fires rules at will | A cooldown per rule and pane, 5 runs per second per pane, 20 per second overall (not for rules that only `highlight`, `marker` or `counter`), per-action limits, counted drops, **Pause all**, and `trigger.fired` events so abuse is visible. No action runs a program, types into a pane or uses the network. |
| A rule puts attacker text on your clipboard | Off by default behind its own gate; capture text only, one sanitised line, 4 KiB, rate limited, never PRIMARY, and a toast on every copy. |
| Captures fill the disk or escape their folder | Own gate, fixed folder, generated names, no symlinks, size and count caps, rotation of microterm's own files only. |
| Secrets in captures | Off by default; text is redacted on a best-effort basis (a safety net, not a guarantee); screenshots cannot be; files are private (`0600`/`0700`) but unencrypted; the panel warns while capture actions are active. |
| Spoofing through titles, notifications, status | The same sanitiser as OSC titles; plain text; the tooltip says a title or color was set by a trigger and names the rule. |
| Another process installs or silences rules | `ctl`, IPC and MCP cannot create, edit or delete rules, and `enable`/`disable`/`pause` last for the session only. A process of yours can still edit `triggers.toml`, as it can already edit `microterm.toml`. |
| A hostile regex | The `regex` crate has no backtracking; size limits; a 4096-character scan cap; a worker that is never on the PTY or UI thread. |
| A rule that triggers itself | No action produces trigger input: `notify` and `status` render in app chrome, `bell` does not raise the bell *event*, and title and color changes publish nothing a rule listens to. |
| A trigger recording captures passwords | The recording action cannot capture input; there is no such parameter. |
