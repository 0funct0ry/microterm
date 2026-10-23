---
title: Exit codes
description: What the microterm command line returns.
---

| Code | Meaning |
|------|---------|
| `0` | success |
| `1` | usage error: bad flags, no target for an input or destructive verb, an inapplicable selector key, invalid input, or any other failure |
| `2` | no running instance: no live socket was found |
| `3` | target not found: unknown pane, tab, window, workspace, recording or file |
| `4` | protocol or version mismatch: the running instance speaks another major protocol version |
| `5` | ambiguous target: several items matched and `--all` was not given; the error lists the candidates |
| `6` | timeout: `pane wait` or `pane run --timeout` ran out of time; the command keeps running |
| `7` | needs [shell integration](/docs/shell-integration/): no OSC 133 marks seen in that pane (`pane run --in`, `pane wait --command-finished`, `pane get-text --last-command`) |
| `8` | confirmation required: `confirm_close` would ask before closing, or `recording rm` was run, and `--force` was not given; nothing was closed or deleted |
| `9` | pane busy: `pane run --in` while a command is already running in that pane; nothing was typed |
| `10` | feature disabled by config: `ctl snippet …` while `snippets.enabled = false`, `ctl recording …` while `recording.enabled = false` |
| `11` | missing values: a [snippet](/docs/snippets/) needs prompt values (`--var`, `--vars-file`) that were not given; the error lists every missing label with its kind |
| `12` | export not possible: `ctl recording export` for a format the recording cannot produce (`vhs` without input events) or an unknown format or invalid `--opt` |

The codes map from the `code` of an IPC error: `bad_request` and `internal` are `1`, `not_found`
is `3`, `version_mismatch` is `4`, `ambiguous` is `5`, `timeout` is `6`, `needs_shell_integration` is `7`,
`confirm_required` is `8`, `busy` is `9`, `disabled` is `10`, `missing_vars` is `11` and `export_impossible` is `12` (`template_error`, a snippet whose template fails to parse or expand, is `1`). `--help` and `--version` exit `0`.

`window close`, `tab close`, `pane close`, `workspace delete` and `recording rm` never block on a dialog: they
either honour `--force` or fail with `8`, so scripts and agents cannot hang.

`pane wait` also exits `3` when the pane closes while it waits (unless it waits for `--exit`).
`pane run --propagate-exit` returns the command's own status instead of `0`; statuses that would
collide with the codes above become `1`. See [Run & wait](/docs/run-and-wait/).

## In `microterm sh`

[`microterm sh`](/docs/sh/) returns the same codes: the status of the last command (or the first
failing one with `-e`), `2` when no instance can be reached at start, and `0` after `exit`. In an
interactive session a close that would exit `8` asks `[y/N]` instead; scripts and `-c` still exit `8`.
`--readonly` rejections are usage errors (`1`).
