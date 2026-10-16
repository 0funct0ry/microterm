---
title: Exit codes
description: What the microterm command line returns.
---

| Code | Meaning |
|------|---------|
| `0` | success |
| `1` | usage error: bad flags, missing `--pane`, invalid input, or any other failure |
| `2` | no running instance: no live socket was found |
| `3` | target not found: unknown pane, tab, window, workspace or file |
| `4` | protocol or version mismatch: the running instance speaks another major protocol version |

The codes map from the `code` of an IPC error: `bad_request` and `internal` are `1`, `not_found`
is `3`, `version_mismatch` is `4`. `--help` and `--version` exit `0`.
