---
title: Exit codes
description: What the microterm command line returns.
---

| Code | Meaning |
|------|---------|
| `0` | success |
| `1` | usage error: bad flags, no target for an input or destructive verb, an inapplicable selector key, invalid input, or any other failure |
| `2` | no running instance: no live socket was found |
| `3` | target not found: unknown pane, tab, window, workspace or file |
| `4` | protocol or version mismatch: the running instance speaks another major protocol version |
| `5` | ambiguous target: several items matched and `--all` was not given; the error lists the candidates |
| `8` | confirmation required: `confirm_close` would ask before closing, and `--force` was not given; nothing was closed |

The codes map from the `code` of an IPC error: `bad_request` and `internal` are `1`, `not_found`
is `3`, `version_mismatch` is `4`, `ambiguous` is `5` and `confirm_required` is `8`. `--help` and `--version` exit `0`.

`window close`, `tab close`, `pane close` and `workspace delete` never block on a dialog: they
either honour `--force` or fail with `8`, so scripts and agents cannot hang. Codes `6`, `7` and
`9` are reserved for `run` and `wait`.
