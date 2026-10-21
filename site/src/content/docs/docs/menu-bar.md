---
title: Menu bar
description: The macOS menu bar - every common action with its live shortcut, greyed out when it cannot apply.
---

On macOS microterm installs a native menu bar with six menus: **microterm**, **Shell**, **Edit**,
**View**, **Window** and **Help**. Every row runs the same action as its key binding or command
palette entry, and shows the shortcut you actually have: rebind an action in `microterm.toml` and
the chip updates after the [hot reload](/docs/hot-reload/). Linux has no global menu bar; use the
[command palette](/docs/command-palette-and-launcher/) and [context menus](/docs/context-menus/).

## Menus

| Menu | Rows |
|---|---|
| microterm | About microterm, Settings…, Reload Config, Services, Hide, Hide Others, Show All, Quit |
| Shell | New Tab, New Window, Split Right / Down / Left / Up, Close Tab, Close Pane, Close Window, Rename Tab, Reset Terminal, Show Launcher, Open Link at Mouse Cursor, New Tab in Domain ▸, Detach Workspace, Attach ▸ |
| Edit | Copy, Paste, Paste Selection, Select All, Find…, Quick Select, Copy Mode, Clear Scrollback |
| View | Command Palette, Increase / Decrease / Reset Font Size, Select Theme…, Toggle Fullscreen, Toggle Pane Zoom, Workspace Switcher |
| Window | Minimize, Zoom, Next / Previous Tab, Go to Tab ▸, Focus Pane ▸, Swap Pane ▸, Rotate Panes, Next / Previous Workspace, Bring All to Front |
| Help | Documentation, Release Notes, Report an Issue, Show Config Folder |

**New Tab in Domain** lists *Local* and one `SSH: <name>` row per `[[ssh_domains]]` entry; it is
rebuilt when the config reloads.

## Greyed-out rows

| Row | Disabled when |
|---|---|
| Close Pane, Toggle Pane Zoom, Focus / Swap Pane, Rotate Panes | the tab has only one pane |
| Copy | nothing is selected |
| Open Link at Mouse Cursor | no link is under the last pointer position |
| Detach Workspace, Next / Previous Workspace | there is only one workspace |
| Attach | no workspace is detached |
| Go to Tab ▸ N | the window has fewer than N tabs |

## Reset Terminal

Resets modes (bracketed paste, mouse reporting, application cursor keys, alternate screen),
charsets, cursor style, text attributes, scroll region, tab stops and program-set colors in the
focused pane. Unlike a full `reset`, the screen and the scrollback are kept.

## Detach and Attach

**Detach Workspace** closes the active workspace's windows and shows another workspace; its shells
keep running. **Attach** lists the detached workspaces and brings the chosen one back with its
shells alive.

## Configuration

```toml
[menu_bar]
enabled = true   # false keeps only the microterm menu, with Quit
```

## From the command line

```sh
microterm ctl menu-bar list
microterm ctl menu-bar list --json | jq '.items[] | select(.enabled | not)'
```

Each row has its path, action, enabled state and shortcut chip. A headless server has no menu bar
and lists nothing.
