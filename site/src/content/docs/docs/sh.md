---
title: Interactive shell
description: microterm sh, a REPL with completion over every microterm ctl command.
---

`microterm sh` is an interactive shell for exploratory work. It offers exactly what
[`microterm ctl`](/docs/ctl/) offers, without retyping the boilerplate: no `microterm ctl` prefix,
IDs and selectors that complete with Tab, a sticky target, history, and tables and trees on a
terminal. It adds no capability and no protocol: a line is `ctl`'s command line, parsed by the same
definitions and run by the same code, so anything `sh` can do `ctl` can do and the other way round.

```sh
microterm sh                      # interactive (needs a terminal)
microterm sh -c 'pane list'       # run commands and exit
microterm sh script.msh           # or: microterm sh < script.msh   (no prompt, no completion)
```

| Flag | Does |
|------|------|
| `-c CMDS` | run these commands and exit |
| `--json` | print JSON, as `ctl --json` |
| `--socket PATH` | use this instance instead of the newest running one |
| `--no-history` | neither read nor write the history file |
| `--no-color` | no colour (`NO_COLOR` is honoured too) |
| `--readonly` | refuse every command that changes state |
| `-e` | abort a script at the first failing command |

The exit status is the exit code of the last command (see [Exit codes](/docs/reference/exit-codes/)),
`2` when no instance can be reached when the shell starts, and `0` after `exit`.

## Typing commands

```text
microterm> pane list
microterm> pane get 7                 # same as: pane get --pane 7
microterm> pane list 'title:*cargo*,!exited'   # a bare selector is --match
microterm> use pane 7
microterm[pane 7]> pane get-text      # the context supplies --pane 7
microterm[pane 7]> cat                # alias for pane get-text
```

- **Aliases:** `ls` = `list`, `ws` = `workspace`, `w` = `window`, `t` = `tab`, `p` = `pane`,
  `type` = `pane send-text`, `cat` = `pane get-text`. Add your own with `alias ll='pane list --json'`.
  An alias expands once, on the first word.
- **Positional target:** `pane get 7`, `tab close 3 --force`, `pane status get 4`. A number is an ID,
  anything else is a selector. For verbs that also take text (`send-text`, `notify`, `tab rename`,
  `status set`) the target is recognised only when a text follows it, so `type 7` types the text `7`.
- **Statements:** `;` runs both, `a && b` runs `b` if `a` succeeded, `a || b` if it failed. A trailing
  `\`, an open quote or a trailing `&&` continues the line.
- **Host commands:** `!date` runs a shell command; `pane get-text 7 | grep error` pipes the output
  of a command into one; `list > tree.txt` and `>>` redirect it. There are no pipelines between
  `sh` commands.
- **Input:** inside `sh`, `send-text` without TEXT is a usage error (it never reads stdin); use
  `-f FILE` or `< FILE` on the host side.

## Context and variables

`use` sets a sticky default target. It ranks below an explicit ID and `--match` and alongside
`$MICROTERM_*`, so `pane get 9` still means pane 9. It is **never** applied to `close`, `delete`
or `send-text`: those always name their target, as with `ctl`.

| Command | Does |
|---------|------|
| `use pane 7`, `use tab 3`, `use window 1` | default target for that resource |
| `use workspace api` | default workspace for `tab new` and `window new` |
| `use --match 'title:build*'` | pin a selector, evaluated again for each command |
| `use` / `use -` | show / clear the context |
| `set NAME=VALUE`, `unset NAME`, `env` | variables; `$NAME`, `${NAME}`, `$?` (last exit code) |

After `tab new`, `window new` or `replay`, `$tab`, `$pane` and `$window` hold the new IDs:

```text
microterm> tab new --title build -- cargo watch -x test
✔ tab 9 created (pane 12, window 1)
microterm> pane wait $pane --match 'Finished' --timeout 120s
```

Other shell commands: `help [command]`, `exit` / `quit` / Ctrl-D, `history`, `json on|off`,
`notify on|off`, `pager on|off`, `refresh`, `source FILE`, `alias`, `reconnect`, `clear`, `version`.
None of them is a `ctl` subcommand, so nothing shadows a `ctl` command.

## Completion and editing

Tab completes, with descriptions, from three layers:

1. **Commands:** resources, verbs, flags and their values, straight from the `ctl` definitions (the
   same source as `microterm completions`), plus the aliases and shell commands.
2. **Live items:** pane, tab and window IDs with `id  title  cwd`, workspace names, domains and file
   paths for `--cwd`, `-o` and `replay`. They come from a cached snapshot that is refreshed when
   microterm reports a change (`refresh` forces it).
3. **Selectors** after `--match` or as a bare argument: keys, `!` negation, `re:` values and the
   values that exist right now. The hint after the cursor says `matches 3 panes`, or points at the
   column of a mistake.

Without a running instance only the first layer works. The line is highlighted (a valid verb green,
an unknown word red, selector keys, strings), Ctrl-R searches the history, and multi-line input
continues until it is complete.

History is stored with mode `0600` in the state directory (`~/Library/Application Support/microterm/`
on macOS, `$XDG_STATE_HOME/microterm/` on Linux), without duplicates and capped. Lines that contain
`send-text`, including `type`, are **not** saved unless `[sh] history_send_text = true`, because they
can carry secrets.

## Output

On a terminal the data is the same as `ctl` prints, laid out for people:

- `*list` verbs are tables with a bold header, right-aligned IDs, a count in the footer, and the
  low-priority `DOMAIN` and `WORKSPACE` columns dropped before text is cut off. Exited panes are dim.
- `pane get` is a key/value card, and `list` a box-drawn tree with `●` for the focused and `▸` for
  the active item.
- IDs are cyan, `✔` confirmations green, `✖` errors red, warnings yellow, and status chips follow
  their state. Only the 16 ANSI colours are used, so your terminal palette applies. `NO_COLOR`,
  `--no-color` and `[sh] color = "never"` turn colour off; the box drawing falls back to ASCII when
  `LANG` is not UTF-8.
- Long output goes through `$PAGER` (`less -R`).

Piped output, redirects, script mode and `--json` are plain: tab-separated rows, raw JSON, and
errors on stderr in the shape described in [Exit codes](/docs/reference/exit-codes/). `sh -c 'pane list' --json`
prints exactly what `ctl pane list --json` prints.

## Connection and events

If the instance goes away the prompt shows `⚠`; the next command connects again, and `reconnect`
does it now. While a notice-worthy event happens (a pane exits, a command finishes, a notification
is raised) a line like `[event] pane 12 exited (status 0)` is printed above the next prompt;
`notify off` silences them. `record` and `replay` run in the foreground and Ctrl-C returns to the
prompt, and Ctrl-C also ends a blocked `pane wait` without killing the command it waits for.

## Safety

`sh` has the same trust boundary as `ctl` (see [Agents & automation](/docs/agents-and-automation/)):
the per-user socket, no listener of its own.

- Destructive verbs keep the exit-`8` contract. In an **interactive** session `sh` asks
  `… Close anyway? [y/N]` and retries with `--force` if you agree; scripts and `-c` never prompt, and
  `[sh] confirm_prompts = false` turns the question off. This is the one intended difference from `ctl`.
- `--readonly` rejects anything that changes state before it reaches the instance (`list`, `get`,
  `get-text`, `wait`, `status get`, `events`, `menu list|show|check|icons` and `menu invoke --dry-run`
  are allowed), and disables `!` host commands.
- `send-text` and `record --input` print a one-line warning the first time they are used in a session.

## Configuration

```toml
[sh]
color = "auto"             # "auto" | "always" | "never"
history_send_text = false  # keep lines containing `send-text` in the history
confirm_prompts = true     # offer the [y/N] question on exit 8 (interactive only)
```

Read when a session starts.
