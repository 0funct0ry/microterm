---
title: Tabs & workspaces
description: Tabs, windows and tmux-style workspaces, startup workspaces and the single-instance behaviour.
---

microterm organises terminals as **workspace → window → tab**. Every tab is a real shell.

## Tabs

| Action | macOS | Linux |
|---|---|---|
| New tab | `⌘T` | `Ctrl+Shift+T` |
| Close tab | `⌘W` | `Ctrl+Shift+W` |
| Next / previous tab | `⌘⇧]` / `⌘⇧[` | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Go to tab 1–9 | `⌘1`…`⌘9` | `Alt+1`…`Alt+9` |
| New window | `⌘N` | `Ctrl+Shift+N` |

`MoveTabRelative`, `RenameTab`, `CloseWindow`, `SpawnTabInDomain` and the workspace actions have no
default keys; bind them under `[[keys]]` or run them from the command palette.

- **Reorder** tabs by dragging them; `MoveTabRelative(1)` / `(-1)` moves the active tab.
- **Rename** with the tab's right-click menu or `RenameTab`. Type the name, `Enter` to keep it,
  `Esc` to cancel. A renamed tab keeps its name; clear the text to go back to the automatic title.
- **Middle-click** a tab to close it. Closing the last tab closes the window.
- A **dot** on a tab means a program other than the shell is running in it.

### Titles

A tab shows its custom name, else the title the program set (OSC 0/2), else the foreground
process. `tab_bar.title_format` shapes the automatic title:

```toml
[tab_bar]
title_format = "{index}: {title}"   # {title} {process} {cwd_basename} {index}
show_index   = false
```

### New tab directory

```toml
new_tab_cwd = "inherit"   # "inherit" | "home" | "/some/path"
```

`inherit` starts the tab in the directory of the active tab's foreground process (read from the
operating system, so it works without any shell integration).

### Closing with programs running

```toml
confirm_close = "process"   # "always" | "process" | "never"
```

`process` asks before closing a tab or window that is running something other than the shell.

## Workspaces

A workspace is a named group of windows, like a tmux session. **One workspace is active at a
time.** Switching hides the windows of the old workspace (their shells keep running) and shows
the windows of the new one. The `default` workspace always exists.

| Action | macOS | Linux |
|---|---|---|
| Workspace switcher | `⌘⌥W` | `Ctrl+Alt+W` |
| Next / previous workspace | `⌘⌥]` / `⌘⌥[` | `Ctrl+Alt+]` / `Ctrl+Alt+[` |

The workspace chip in the title bar shows the active workspace and opens the switcher, which lists
every workspace with its window and tab counts. **Create workspace…** asks for a name and switches
to it; `CreateWorkspace` and `RenameWorkspace` do the same from the palette; `SwitchWorkspace`
takes a name (`action = { SwitchWorkspace = "infra" }`).

Closing the last window of the active workspace shows another workspace that still has windows
before microterm quits.

### Declared workspaces

```toml
startup_workspace = "api"

[[workspaces]]
name = "api"
tabs = [
  { title = "server", cwd = "~/src/api", command = ["cargo", "run"] },
  { title = "logs",   cwd = "~/src/api" },
]

[[workspaces]]
name = "infra"
tabs = [{ title = "ssh", command = ["ssh", "prod"] }]
```

Declared workspaces appear in the switcher at once and start their tabs the first time you switch
to them. `microterm --workspace infra` starts straight in a workspace (it is created if it is not
declared). `--cwd DIR` and `-e CMD…` apply to the first tab.

## One instance

Running `microterm` while it is already running opens a new window in the running instance
(honouring `--workspace`, `--cwd` and `-e`) and exits. Pass `--new-instance` to start a separate
process. The instance listens on a private Unix socket (mode 0700, same-user only) in
`$XDG_RUNTIME_DIR/microterm/` (`~/Library/Application Support/microterm/sock/` on macOS);
`MICROTERM_UNIX_SOCKET` points a client at a specific one.

## Window size

With `window.remember_size = true` (the default) the size of the last resized window is saved in
`$XDG_STATE_HOME/microterm/` (`~/.local/state/microterm/`) and used for new windows.
