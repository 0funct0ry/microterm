# ADR 0001 — Agent integration (skill installer and MCP server)

Status: accepted (M10.4). Checked: 2026-10-19.

## Where each agent reads instructions

| Agent | Finding | Source |
|---|---|---|
| Claude Code | Personal skills `~/.claude/skills/<name>/SKILL.md`, project skills `.claude/skills/<name>/SKILL.md`; YAML frontmatter with `name` and `description`; unknown fields ignored. | <https://code.claude.com/docs/en/skills> |
| Codex | Global `AGENTS.md` in `$CODEX_HOME` (default `~/.codex`; `AGENTS.override.md` wins if non-empty); project `AGENTS.md` walked from the repo root down; combined size capped (`project_doc_max_bytes`, 32 KiB). | <https://learn.chatgpt.com/docs/agent-configuration/agents-md> |
| Antigravity | **Not verified against Google's own documentation.** Only third-party guides were found (they describe `.agents/rules`, `.agents/skills/<name>/SKILL.md` and `~/.gemini/GEMINI.md`, and root `AGENTS.md` since IDE 1.20.5). | <https://thepromptshelf.dev/blog/google-antigravity-agents-md-rules-guide-2026> (unofficial) |
| generic | Root `./AGENTS.md` is the cross-agent convention. | <https://agents.md> |

Decisions:

- `claude` owns the directory `…/skills/microterm-ctl/` (SKILL.md).
- `codex` user scope honours `$CODEX_HOME`; both `codex` and `generic` use a managed block between
  `<!-- microterm:begin -->` and `<!-- microterm:end -->`.
- `antigravity` **falls back to `generic`** (project `./AGENTS.md`, which Antigravity also reads
  according to the unofficial sources) and prints a note saying so. Revisit once Google documents it.
- `microterm ctl skill install --agent all` installs claude + codex + generic (antigravity is
  covered by generic).

## MCP server: hand-written JSON-RPC, not `rmcp`

The server needs `initialize`, `notifications/initialized`, `ping`, `tools/list` and `tools/call`
over newline-delimited stdio. A hand-written loop is ~300 lines, synchronous, and reuses the
existing blocking IPC `Client`. `rmcp` would add tokio and a proc-macro stack to the GUI binary
(SPEC: the GUI path must not gain a heavy runtime). Tool definitions live in `mt_proto::tools`
next to the method/param types; tests round-trip every tool's arguments through the serde param
structs, so schema and protocol cannot drift. No `schemars` dependency.

MCP client config: Claude Code `claude mcp add microterm -- microterm ctl mcp`; Codex
`[mcp_servers.microterm]` with `command = "microterm"`, `args = ["ctl", "mcp"]` in
`~/.codex/config.toml` (Codex's MCP page was not fetched; the table format is the documented
convention and should be re-checked).

## Security

No access controls are added (SPEC §12.6); the docs state the trust model plainly.
