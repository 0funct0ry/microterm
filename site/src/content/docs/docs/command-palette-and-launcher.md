---
title: Command palette & launcher
description: Run any action, jump to workspaces, tabs and themes, and start launch profiles from the keyboard.
---

## Command palette

Press `⌘⇧P` (macOS) or `Ctrl+Shift+P` (Linux). Type to fuzzy-search; matched characters are
highlighted and each action shows its keybinding.

A one-character prefix picks a provider:

| Prefix | Provider | Example | Result |
|---|---|---|---|
| `>` (default) | Actions | `new tab` | Runs **New tab** |
| `@` | Workspaces | `@infra` | **Switch to workspace: infra** |
| `#` | Tabs | `#cargo` | **Go to tab: cargo watch**, with its [status](/docs/notifications-and-status/) (dot and text) when one is set |
| `~` | Themes | `~tokyo` | **Theme: Tokyo Night**, previewed live |

Moving the selection over a theme previews it; `Esc` restores the previous theme, `Enter` keeps it.

Results are ranked by fuzzy score, then by recency: the last 50 items you ran are remembered in
`$XDG_STATE_HOME/microterm/mru.json` (`~/.local/state/microterm/mru.json` by default), so the
things you use most float to the top.

`Enter` runs the highlighted row. `⌘Enter` / `Ctrl+Enter` runs it in a new window where that
makes sense (launch profiles and *New tab in…*).

With [snippets](/docs/snippets/) enabled, `!` lists your snippets ("Snippet: Tail pod logs"; the
row shows the folder and first line). `Enter` inserts the snippet and `⌥Enter` / `Alt+Enter`
inserts and runs it. The provider is absent while `snippets.enabled = false`.

Every action in the [actions reference](/docs/reference/actions/) is reachable from the palette,
except the ones that need an argument (`SendString`, `SpawnCommand`, …): bind those under
`[[keys]]`.

## Launcher

`⌘⇧L` / `Ctrl+Shift+L`, or right-click the `+` button in the tab bar. It lists:

- your `[[launch]]` profiles,
- **New tab in …** for each domain (`local`, then your [SSH domains](/docs/ssh-domains/) with a connection dot), plus *Connect to host…*,
- workspaces.

```toml
[[launch]]
name = "htop"
command = ["htop"]
cwd = "~"

[[launch]]
name = "api logs"
command = ["tail", "-f", "/var/log/api.log"]
```

Edits to `[[launch]]` appear in the launcher right after the config hot-reloads, even if the
launcher is open. A profile with no `command` opens your shell in `cwd`.
