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

- A **status dot**, a thin **progress line** along the bottom edge and an **attention badge** appear on a tab when a process uses `ctl pane status` or `ctl pane notify`; the status text is in the tab's tooltip. See [Notifications & status](/docs/notifications-and-status/).
- **Reorder** tabs by dragging them; `MoveTabRelative(1)` / `(-1)` moves the active tab.
- **Rename** with the tab's right-click menu or `RenameTab`. Type the name, `Enter` to keep it,
  `Esc` to cancel. A renamed tab keeps its name; clear the text to go back to the automatic title.
- **Middle-click** a tab to close it. Closing the last tab closes the window.
- A **dot** on a tab means a program other than the shell is running in it.

### Tab colors

Color a tab so one glance at the tab bar tells prod from the build from the agent. Right-click a
tab: below **Rename…**, the **Tab Color** heading shows ten round swatches in two rows of five.
The first, a hollow ring, is *Default* and clears the manual color; the current color carries a
check mark. Arrow keys move across the grid, `Enter` picks, `Esc` closes. The command palette has
**Tab: Set color…** (a list with a swatch per row) and **Tab: Clear color**, and `ctl` sets colors
from scripts:

```sh
microterm ctl tab color --match 'title:*build*' orange
microterm ctl tab color '#f59e42'          # a custom color, lightness-clamped for contrast
microterm ctl tab color --clear
```

The nine names are `blue`, `purple`, `pink`, `red`, `orange`, `yellow`, `green`, `teal` and `gray`.
They resolve through the theme (see [Themes](/docs/themes/)), so they stay legible in every theme.

A tab's color comes from the first of these that is set:

| Order | Source | Set by |
|---|---|---|
| 1 | `manual` | the menu, the palette, `ctl tab color`, `tab new --color` |
| 2 | `trigger` | a [trigger](/docs/triggers/) `tab_color` action |
| 3 | `declared` | a program (`OSC 7719`, see below), else `color = "…"` on a `[[workspaces]]` tab, a `[[launch]]` profile or an `[[ssh_domains]]` entry |
| 4 | `rule` | the first matching `[[tab_bar.color_rules]]` entry |

The *Default* swatch and `--clear` remove only the manual color, so declared colors and rules apply
again. There is deliberately no way to force "no color" over a declared one: remove the
declaration instead.

#### Declared by a program

With `[private_osc] enabled = true`, a program can color its own tab, for example a login script on
a production host: `printf '\e]7719;tab;color;orange\a'` (an empty value clears). It ranks below
a manual and a trigger color and above config colors and rules, and the tab tooltip reads
`Color: orange (declared by pane 2)`. See [Private OSC](/docs/private-osc/).

```toml
[tab_bar]
color_style = "bar+tint"        # bar | tint | bar+tint | dot | none

[tab_bar.palette]               # override a built-in color, or add up to 10 more names
orange = "#f59e42"
brand = "#3b82f6"

[[tab_bar.color_rules]]         # first match wins; a tab matches if any of its panes does
match = "domain:prod-*"
color = "red"

[[ssh_domains]]
name = "prod-db"
host = "db.example.com"
color = "red"                   # every tab opened in this domain
```

`color_style` picks the look: `bar` is a 2 px line on the tab's top edge (60 % on inactive tabs),
`tint` mixes the color into the tab background (14 % active, 7 % inactive), `bar+tint` is the
default, `dot` is a small dot before the title for dense setups, and `none` stores and reports
colors without drawing them. Status dots, the attention badge and the progress line keep their own
colors; when a status dot would match the tab color it gets a thin ring. The tab tooltip says
`Color: red (rule: domain:prod-*)`.

Color is never the only signal: the tooltip, the palette row and `ctl tab list` all carry the
color name. (GPUI 0.2.2 has no accessibility API yet, so screen-reader labels wait for it, as for
the other menus.)

Remote tabs keep their manual color on the mux daemon, so it survives a reconnect. Local tabs
keep it for the life of the process; rules and declared colors re-apply on every start.

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

[[workspaces.tabs]]
title = "server"
cwd = "~/src/api"
command = ["cargo", "run"]

[[workspaces.tabs]]
title = "logs"
cwd = "~/src/api"

[[workspaces]]
name = "infra"

[[workspaces.tabs]]
title = "ssh"
command = "ssh prod"      # a string is split like a shell would; an array is canonical
```

The nested array-of-tables form above is the recommended spelling (inline `tabs = [ … ]` still
works). `cwd` on a tab or a split node is inherited by every descendant pane that does not set its
own.
Declared workspaces appear in the switcher at once and start their tabs the first time you switch
to them. `microterm --workspace infra` starts straight in a workspace (it is created if it is not
declared). `--cwd DIR` and `-e CMD…` apply to the first tab. Tabs can start as
[split panes](/docs/panes-and-splits/#declared-splits).

### Workspace designer

Instead of typing nested TOML, run **Design Workspace…** from the command palette (or
**Edit workspace…** in the workspace switcher). The modal lists your workspaces (★ marks the
startup workspace; New, Duplicate and Delete edit the list), the selected workspace's name and tab
strip, a split canvas with Split right / Split down / Close pane / Swap and a depth counter, and
an inspector for the selected pane's `cwd` and `command`. Arrow keys nudge the selected pane's
divider in steps of 5. **TOML preview** shows exactly what Save will change. Each control maps
one-to-one onto the `[[workspaces]]` config, and the inspector's path label is the TOML path.

**Save to config** rewrites only `[[workspaces]]` and `startup_workspace`, in the nested form;
the rest of `microterm.toml` and any untouched workspace stay byte-identical. Comments inside a
workspace that was rewritten are lost, and the previous file is kept as `microterm.toml.bak`. If
the file changed on disk while the designer was open, or does not parse, Save refuses. Save is
disabled while the layout is invalid. New workspaces appear in the switcher at once; edits to an
existing workspace apply when its windows are next created. Running workspaces are never torn down.

**Save current layout as workspace…** opens the designer seeded from the active tab's pane tree
(cwds and split ratios; commands are left empty).

## Detach and attach

**Shell ▸ Detach Workspace** (action `DetachWorkspace`) parks the active workspace, with its shells
running, and shows another one. **Shell ▸ Attach ▸ name** (`AttachWorkspace`) brings a detached
workspace back. See [Menu bar](/docs/menu-bar/).

## One instance

Running `microterm` while it is already running opens a new window in the running instance
(honouring `--workspace`, `--cwd` and `-e`) and exits. Pass `--new-instance` to start a separate
process. The instance listens on a private Unix socket (mode 0700, same-user only) in
`$XDG_RUNTIME_DIR/microterm/` (`~/Library/Application Support/microterm/sock/` on macOS);
`MICROTERM_UNIX_SOCKET` points a client at a specific one.

## Window size

With `window.remember_size = true` (the default) the size of the last resized window is saved in
`$XDG_STATE_HOME/microterm/` (`~/.local/state/microterm/`) and used for new windows.
