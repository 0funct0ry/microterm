---
title: Panes & splits
description: Split a tab into panes, move focus with the keyboard or the mouse, zoom, resize and script the layout.
---

A tab can hold several **panes**, each a real shell, arranged as a tree of splits. Splits work in
local tabs and in [SSH domains](/docs/ssh-domains/); a pane you split off an SSH pane runs on the
same host.

## Keys

| Action | macOS | Linux |
|---|---|---|
| Split right (side by side) | `⌘D` | `Ctrl+Shift+D` |
| Split down (stacked) | `⌘⇧D` | `Ctrl+Shift+E` |
| Focus pane left / right / up / down | `⌘⌥←` `→` `↑` `↓` | `Ctrl+Alt+←` `→` `↑` `↓` |

The new pane takes focus, starts in the directory `new_tab_cwd` says and shares the domain of the
pane you split. Focus moves to the nearest pane in that direction (the one sharing the longest
edge when two are equally near) and stops at the edge of the tab; it never wraps.

More actions are in the [command palette](/docs/command-palette-and-launcher/) and can be bound
under `[[keys]]` (see the [action reference](/docs/reference/actions/)):

| Action | What it does |
|---|---|
| `ClosePane` | Close the focused pane. The last pane of a tab closes the tab. |
| `TogglePaneZoom` | Show the focused pane full-size; again to restore. Moving focus to another pane, or splitting, leaves zoom. |
| `ResizePane` | Move the focused pane's border: `{ ResizePane = { direction = "left", cells = 5 } }`. |
| `SwapPane` | Swap with the neighbour: `{ SwapPane = "left" }`. |
| `RotatePanes` | Rotate every pane one position. |

```toml
[[keys]]
key = "cmd+shift+enter"
action = "TogglePaneZoom"

[[keys]]
key = "cmd+ctrl+left"
action = { ResizePane = { direction = "left", cells = 3 } }
```

`confirm_close` applies to `ClosePane` like it does to `CloseTab`: with `"process"`, a pane that
runs a program asks first.

## Mouse

- **Click** a pane to focus it. The first click only focuses; it is not sent to the program.
- **Drag a divider** (a 1 px line with a 6 px grab area) to resize the two sides of the split.
- The **wheel** scrolls the pane under the pointer without changing focus.
- Unfocused panes are dimmed (`panes.inactive_dim`) and the focused one has a thin accent border.
  A zoomed tab shows a single pane without dividers.

### Focus follows mouse

```toml
[mouse]
focus_follows_mouse = true

[panes]
inactive_dim = 0.15     # 0.0 to 1.0
focus_delay_ms = 0      # wait this long before the hovered pane takes focus (0 to 5000)
```

With `focus_follows_mouse`, moving the pointer onto another pane focuses it after
`focus_delay_ms`. Moving back before the delay ends cancels the switch. It does nothing while an
overlay or menu is open, or during a selection or divider drag. Quick select, copy mode and
search always act on the focused pane, and focus moving to another pane ends copy mode and quick
select. Both settings reload live.

## Declared splits

A tab in `[[workspaces]]` becomes a split when it has `split = "horizontal"` (side by side) or
`"vertical"` (stacked) and exactly two `panes`. A pane is either a leaf (`cwd`, `command`) or a
split itself, up to four levels deep. `percent` is the share of the **first** pane (default 50).

```toml
[[workspaces]]
name = "api"
tabs = [
  { title = "dev", split = "horizontal", percent = 60, panes = [
      { cwd = "~/src/api", command = ["nvim"] },
      { split = "vertical", panes = [
          { command = ["cargo", "watch", "-x", "run"] },
          { command = ["htop"] },
      ] },
  ] },
  { title = "shell", cwd = "~/src/api" },
]
```

The first leaf starts focused. `--cwd` and `-e` on the command line replace the first tab,
splits included.

## From the command line

```sh
microterm ctl pane split --horizontal                  # split the pane you are in
eval "$(microterm ctl pane split --vertical --percent 30 -- htop)"   # sets tab= pane= window=
microterm ctl pane focus --direction left
microterm ctl pane zoom
microterm ctl pane resize --direction right --cells 10
microterm ctl pane swap --direction up
microterm ctl pane rotate
```

`ctl list` shows each split tab's layout, `1 | (2 / 3)` meaning pane 1 beside a column of panes 2
and 3 (`|` side by side, `/` stacked, `|40` when the first pane's share is not 50%). With `--json`
a tab carries `layout` (`{"type":"split","axis":"horizontal","percent":50,"a":…,"b":…}` or
`{"type":"leaf","pane":3}`) and each pane has `zoomed`. `pane run --in`, `send-text` and
`get-text` work on any pane by ID or selector. `pane focus --direction` and `pane swap` exit 3
when there is no pane that way. Events: `pane.focused` and `tab.layout_changed`.

## Remote panes

Splitting an SSH pane starts a new shell on the same host. The daemon records how each pane was
split off another (`Split` in the [mux protocol](/docs/microterm-mux/)). The layout itself lives
in the window, so a dropped connection changes nothing: after reconnecting, every pane re-attaches
with its screen and process intact and the split stays as it was.

:::note
A new microterm window cannot yet reopen the tabs of an earlier one, so the daemon's split
records are kept for that future use; they are not shown anywhere today.
:::
