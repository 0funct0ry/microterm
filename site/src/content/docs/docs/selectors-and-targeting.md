---
title: Selectors & targeting
description: How microterm ctl finds the pane, tab or window a command acts on.
---

Every `microterm ctl` command that acts on an item resolves it the same way. This page is the
whole story: the resolution order, the selector language and what happens when zero or several
items match.

## Resolution order

The first rule that applies wins:

1. **An explicit ID**: `--pane ID`, `--tab ID` or `--window ID`.
2. **A selector**: `--match EXPR`.
3. **Your own context.** A process running in a pane inherits `$MICROTERM_PANE`,
   `$MICROTERM_TAB` and `$MICROTERM_WINDOW`, so `microterm ctl tab close --force` or
   `microterm ctl pane get-text` with no flags mean *this* tab or pane.
4. **The focused item**, for **read-only** verbs only (`list`, `get`, `get-text`).

Input and destructive verbs (`send-text`, `close`, `delete`, `rename`, `focus`) never fall back to
the focused item: with no target they fail with exit code **1**. A script in some other terminal
must not type into, or close, whatever pane happens to have focus.

An ID that does not exist, for instance a stale `$MICROTERM_TAB` after the tab was closed, is
"not found" (exit 3).

## Selector syntax

```text
EXPR   := CLAUSE (',' CLAUSE)*          clauses are ANDed
CLAUSE := ['!'] KEY ':' VALUE           '!' negates the clause
VALUE  := glob                          * matches any run of characters, ? one character
        | 're:' REGEX                   Rust regex syntax, unanchored
```

Globs match the whole value. Because a comma separates clauses, write a literal comma inside a
regex as `\x2c`.

| Key | Applies to | Matches |
|-----|-----------|---------|
| `id` | all | numeric ID |
| `title` | pane, tab, window | the effective title |
| `cwd` | pane | working directory (`~` is expanded) |
| `cmd` | pane | the foreground process |
| `domain` | pane, tab | domain name (`local`, `prod`, …) |
| `workspace` | window, tab, pane | workspace name |
| `window`, `tab` | tab, pane (`tab` only pane) | parent ID |
| `focused` / `active` | pane, tab, window | `true` / `false` |
| `status` | pane | status state: `idle` (also no status), `working`, `waiting`, `done`, `error`; see [Notifications & status](/docs/notifications-and-status/) |
| `exited` | pane | `true` / `false` |
| `color` | pane (its tab's), tab | the resolved color: a name (`red`) or `#rrggbb`; see [Tab colors](/docs/tabs-and-workspaces/#tab-colors) |

A key that does not apply to the resource is a usage error (exit 1), never an empty match. A
pane with no value for a key (no `cwd` yet, say) does not match a positive clause and does match
a negated one.

```sh
microterm ctl tab close --match 'title:build*,domain:local' --force
microterm ctl pane get --match 'cmd:re:^cargo (watch|run),!exited:true'
microterm ctl pane list --match 'workspace:api,!focused:true'
```

`focused` is true for the active tab of the focused window (and its pane); `active` is true for
the active tab of any window. A window is `active` when it belongs to the active workspace.

## How many matches

- `list` verbs return every match, and zero matches is an empty list.
- Acting verbs need **exactly one**. Zero matches exits **3**. Several exits **5**, and the error
  lists the candidates so you can narrow the selector:

```text
microterm: 2 tabs match "title:build*"; narrow the selector or pass --all
  tab 4 "build-api" (window 2)
  tab 7 "build-web" (window 2)
```

- `--all` applies the verb to every match. It is accepted by `close` and `send-text`.

## Output and errors

With `--json`, stdout is one JSON value. Errors go to stderr as one JSON object:

```json
{"error":{"code":"ambiguous","message":"2 tabs match \"title:build*\"; …","candidates":["tab 4 …","tab 7 …"]}}
```

`code` is one of `bad_request`, `not_found`, `version_mismatch`, `ambiguous`,
`confirm_required`, `no_instance`. See [Exit codes](/docs/reference/exit-codes/).
