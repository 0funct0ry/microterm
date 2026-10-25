---
title: Agents & automation
description: Let Claude Code, Codex and other coding agents drive microterm tabs, as a skill or as MCP tools.
---

`microterm ctl` is built to be driven by coding agents. Every command is non-interactive and never
waits on a dialog, `--json` output is stable, exit codes are deterministic ([Exit
codes](/docs/reference/exit-codes/)), returned output is bounded, and a process inside a pane can
address its own pane through `$MICROTERM_PANE`, `$MICROTERM_TAB` and `$MICROTERM_WINDOW` without
knowing any ID.

There are two ways to give an agent access. Use either or both.

## Patterns

| Need | Commands |
|---|---|
| Run a command and capture its result | `microterm ctl pane run --json -- cargo test` |
| Start a dev server, wait until ready, read errors later | `microterm ctl pane run --detach --title dev -- npm run dev`, then `microterm ctl pane wait --match title:dev --match 'ready on' --timeout 120s`, then `microterm ctl pane get-text --match title:dev` |
| Drive an interactive program (REPL, `ssh`, debugger) | `pane send-text` + `pane wait --idle 1s` + `pane get-text` |
| Ask a human | `microterm ctl pane status set --state waiting "approve migration?"` + `microterm ctl pane notify --level attention …` |
| Fan out parallel work | one `microterm ctl tab new --title task-N -- …` per task; `microterm ctl events --type pane.command_finished --match 'title:task-*'` to collect |
| Clean up | `microterm ctl tab close --match 'title:task-*' --all --force` |
| Reuse the user's saved [snippets](/docs/snippets/) | `microterm ctl snippet list --json` (ids and the prompts each asks for), then `microterm ctl snippet insert ID --pane $MICROTERM_PANE --var Label=value --run` |

See [Run & wait](/docs/run-and-wait/), [Notifications & status](/docs/notifications-and-status/) and
[Selectors & targeting](/docs/selectors-and-targeting/). For `pane run --in` and
`get-text --last-command`, enable [shell integration](/docs/shell-integration/).

## Option 1: install the skill

```sh
microterm ctl skill install --agent claude      # ~/.claude/skills/microterm-ctl/SKILL.md
microterm ctl skill install --agent claude --scope project   # ./.claude/skills/microterm-ctl/
microterm ctl skill install --agent codex       # managed block in ~/.codex/AGENTS.md ($CODEX_HOME)
microterm ctl skill install --agent generic     # managed block in ./AGENTS.md
microterm ctl skill install --agent all         # claude + codex (user) + generic (project)
```

Add `--print` to see where it would write, `skill show --agent X` to print the text, and
`skill uninstall` with the same flags to remove exactly what was installed. Re-running install
changes nothing when the text is current.

- **Claude Code** gets a skill directory owned by microterm.
- **Codex** and **generic** get a block between `<!-- microterm:begin -->` and
  `<!-- microterm:end -->` in `AGENTS.md`. Everything outside the block is left untouched, byte for
  byte. If `AGENTS.override.md` exists next to the Codex file, Codex reads that instead.
- **Antigravity**: its instruction location could not be verified against Google's
  documentation, so `--agent antigravity` writes the generic `./AGENTS.md` block and says so. The
  reasoning and sources are in `docs/adr/0001-agent-integration.md`.

The installed text is the same one published as the [Agent skill
reference](/docs/reference/agent-skill/), rendered from the `ctl` command definitions.

## Option 2: the MCP server

`microterm ctl mcp` speaks the Model Context Protocol on stdio, for harnesses that prefer tools to
shell commands. It needs a running microterm (it finds the socket like any `ctl` command) and
returns the same JSON as `--json`.

```sh
# Claude Code
claude mcp add microterm -- microterm ctl mcp
```

```toml
# Codex: ~/.codex/config.toml
[mcp_servers.microterm]
command = "microterm"
args = ["ctl", "mcp"]
```

```json
{ "mcpServers": { "microterm": { "command": "microterm", "args": ["ctl", "mcp"] } } }
```

| Tool | Does |
|---|---|
| `microterm_list` | the workspace, window, tab and pane tree |
| `microterm_tab_new` / `microterm_tab_close` | open or close tabs |
| `microterm_pane_run` | run a command (new tab, or typed into a shell pane) and return output and status |
| `microterm_pane_wait` | wait for a regex, idle, exit or a finished command |
| `microterm_pane_read` | screen text, scrollback ranges or the last command's output |
| `microterm_pane_send_text` / `microterm_pane_close` | type into or close panes |
| `microterm_notify` / `microterm_status_set` / `microterm_status_clear` | tell the human what is going on |
| `microterm_events_next` | long-poll for events (`timeout_ms`, default 30 s, max 120 s) |

Failures come back as tool errors whose text is the `ctl` error object plus its `exit_code`. If the
MCP server runs inside a microterm pane, tools default to that pane like the CLI does.

### Snippet tools

While `snippets.enabled = true` the server also lists `microterm_snippet_list`,
`microterm_snippet_expand` and `microterm_snippet_insert` (it checks on every `tools/list`, so they
appear and disappear with the config). Give a value in `vars` for every prompt that
`microterm_snippet_list` reports; a missing one fails with `missing_vars` (exit code 11) and the
labels. Snippets that ask for a `secret` are always refused over MCP, so secrets never pass
through an agent. `insert` needs an explicit pane like `microterm_pane_send_text`.

### Recording tools

While `recording.enabled = true` the server also lists `microterm_recording_list`,
`microterm_recording_start`, `microterm_recording_stop` and `microterm_recording_export`
(checked on every `tools/list`, like the snippet tools). `start` records a pane (default: the
caller's own) **without** capturing typed input unless the call passes `input: true`, because
keystrokes can contain secrets. `stop` takes a `pane`, a recording `id` or `all`. `export` takes
an `id`, a `format` (`cast` or `vhs`), an optional `output` path, an `opts` object with the same
keys as `--opt` and `force`; a recording that cannot be exported in that format fails with exit
code 12.

### Triggers are not available to agents

There is no MCP tool for [triggers](/docs/triggers/), and `ctl trigger` cannot create, edit or
delete a rule: rules that react to terminal output are written by you in the panel or in
`triggers.toml`. `ctl trigger enable|disable|pause|resume` change only the running session, and
`ctl events` shows `pane.bell` and `trigger.fired` so you can watch what rules do.

### The private OSC master switch is not available to agents

There is no MCP tool for the [private OSC](/docs/private-osc/) range, and `ctl private-osc` has
`status` and `disable` but no `enable`. Turning the range on is done by you, in `microterm.toml`
or with the **Toggle private OSC** action; a program in a pane, a remote host or an agent cannot
do it. `ctl events` reports `private_osc.changed` (the feature name, never its content).

## Security: read this before enabling it

`ctl`, the socket and the MCP server are, by design, remote control of every pane. The only
protection in v1 is that the socket is private to your user. These gaps are known and documented
rather than hidden:

| What can go wrong | In plain language |
|---|---|
| Any process running as you can use the socket | `pane send-text` and `pane run` are arbitrary command execution, including in panes connected to production over SSH. A compromised dependency or stray script in one pane can drive the others. |
| Child processes inherit the socket path | Installers, build scripts and agents started in a pane receive full access without being asked. |
| Output is readable | Secrets printed or typed with echo on in any pane can be read with `get-text`, `run`, `wait` or `events`. |
| Terminal text is untrusted input to agents | A file, web page or log line read through `get-text` can contain instructions. An agent that follows them can use `ctl` against other panes. |
| `status` and `notify` text is attacker-influenced | It can spoof an "approve?" prompt. Control characters are stripped, length is capped and notifications are rate limited. |
| The skill and MCP server extend all of the above to the agent that loads them | The installer only writes its own directory or block, prints what it wrote, and `skill uninstall` reverses it. |
| The socket is never forwarded into SSH panes | `MICROTERM_UNIX_SOCKET` is not exported to SSH-domain panes, so a remote host cannot reach your GUI. |
| There is no audit log | After an incident there is no record of who typed what. |
| `record --input` captures keystrokes | Including typed secrets. It is opt-in. |

Planned for a later hardening release: an `allow_ctl` setting, per-pane scopes and tokens, approval
prompts for input to remote panes, and an audit log. Until then, treat anything you give `ctl`
access to as having your shell.
