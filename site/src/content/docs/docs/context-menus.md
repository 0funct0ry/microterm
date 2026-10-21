---
title: Context menus
description: Right-click menus you can extend from the config, act on the pane you clicked, open from the keyboard and run from the command line.
---

Every right-click menu in microterm is a **named menu**: `viewport` (right-click in a pane) and
`tab` (right-click on a tab header). Each has built-in items, and you can add your own from
`microterm.toml`, prune the built-ins, and run any item from a key, the command palette or
`microterm ctl`. The [generated reference](/docs/reference/context-menus/) lists what each menu
offers.

## Using the menus

- **Right-click a pane** to open its `viewport` menu. The menu acts on the pane you clicked, not
  the focused one, and opening it does not move focus. A link or a selection under the pointer
  enables Open link, Copy link, Copy and Search.
- The built-in **Panes** group offers Toggle zoom, Swap ▸ left/right/up/down, Rotate, Split
  left/right/up/down and Close pane. Swap and Rotate appear when the tab has more than one pane.
  The new pane of a split goes on the side you picked and takes focus.
- **Keyboard**: ↑/↓ move, → opens a submenu and ← closes it, Enter runs, Esc closes. Typing
  filters the rows (submenu items included); Backspace edits the filter and Esc clears it first.
- `ShowContextMenu` opens a menu without the mouse (at the cursor, for the focused pane):

```toml
[[keys]]
key = "cmd+shift+m"
action = { ShowContextMenu = "viewport" }
```

When a program uses the mouse (vim, tmux, htop), plain right-click goes to the program; hold
**Shift** to get the menu.

## Adding items

```toml
[context_menu.viewport]
mode = "append"              # "append" (after the built-ins), "prepend" or "replace"
builtin = ["copy", "paste", "panes"]   # optional: keep only these built-ins, in this order
max_rows = 20                # longer menus scroll
trigger = "right_click"      # "right_click" | "shift_right_click" | "paste" | "none"

[[context_menu.viewport.items]]
id    = "blame"              # optional; defaults to a slug of the label
label = "git log -S {selection}"
when  = "selection"
spawn = { command = ["git", "log", "-S", "{selection}", "--oneline"], cwd = "{cwd}", where = "split-right" }

[[context_menu.tab.items]]
label = "Duplicate tab"
spawn = { command = ["sh"], cwd = "{cwd}", where = "tab" }
```

In the `viewport` menu a separator divides the built-in rows from yours (after the built-ins with
`mode = "append"`, before them with `"prepend"`).

`mouse.right_click` (`"context_menu"`, `"paste"`, `"none"`) still works and means the same as
`context_menu.viewport.trigger`; if you set both, `trigger` wins and the config banner says so.

### Item kinds

Each item has exactly one of these keys:

| Kind | Example | What it does |
|---|---|---|
| `action` | `action = "RotatePanes"`, `action = { Split = "left" }` | Any [action](/docs/reference/actions/), in the same form as `[[keys]]`. It runs on the clicked pane (focusing it first). |
| `send` | `send = "git show {word}\n"` | Types the text into the pane. Values are shell-quoted unless `quote = "none"`. |
| `spawn` | `spawn = { command = ["htop"], where = "split-down" }` | Runs a program in a **new pane**: `where = "tab"` (default), `"split-left"`, `"split-right"`, `"split-up"`, `"split-down"` or `"window"`. The pane stays open after the program exits unless `close_on_success = true` and it exited 0. |
| `ctl` | `ctl = "pane status set --state waiting review"` | Any [`microterm ctl`](/docs/ctl/) command, run as the clicked pane. The result shows as a toast. |
| `submenu` | `submenu = [ { label = "Zoom", action = "TogglePaneZoom" } ]` | One level of nesting. |
| layout | `type = "separator"`, `{ type = "header", label = "Git" }` | A divider or a heading. |
| `include` | `include = "layout"` | Inserts the items of `[[menu_groups.layout.items]]`. |

There is no way to run a hidden background process: everything a menu starts is visible in a
pane, where you can see its output and stop it.

Other item keys: `icon` (a bundled Lucide icon name such as `"search"`, `"git-branch"` or `"file"`; see them in the [icon gallery](/docs/reference/context-menus/#icons) or run `microterm ctl menu icons`), `when`,
`when_false = "hide" | "disable"`, `confirm`, and for `send`, `quote` and `multiline`.

### Reusable groups

```toml
[[menu_groups.layout.items]]
label = "Zoom"
action = "TogglePaneZoom"

[[menu_groups.layout.items]]
label = "Split left"
action = { Split = "left" }

[[context_menu.viewport.items]]
include = "layout"

[[context_menu.tab.items]]
include = "layout"
```

A group is checked against every menu that includes it, so a group that uses `{selection}` loads
in `viewport` but is an error in `tab`.

## `when`: show items only where they apply

`when` uses the [selector](/docs/selectors-and-targeting/) syntax, with more keys. Clauses are
separated by commas and all must hold; `!` negates one. A bare key means "present and true";
numbers compare with `=`, `!=`, `>`, `>=`, `<`, `<=`.

| Keys | Meaning |
|---|---|
| `title`, `cwd`, `cmd`, `domain`, `status`, `exited`, `focused` | The pane, as in selectors |
| `link`, `selection`, `word`, `line` | What was right-clicked (viewport only) |
| `on_prompt`, `in_output` | The clicked row is a prompt / command output ([shell integration](/docs/shell-integration/)) |
| `alt_screen`, `mouse_reporting` | A full-screen program is running / owns the mouse |
| `panes`, `tabs`, `zoomed`, `remote`, `local` | The tab and window |
| `last_exit`, `idle`, `running` | The last command's status, and whether one is running (shell integration) |

```toml
when = "panes>1"
when = "link:re:^https?://"
when = "last_exit!=0, idle"
when = "!remote, selection"
```

A false `when` hides the item, or greys it out with `when_false = "disable"`.

## Placeholders

`{selection}` `{word}` `{line}` `{link}` `{path}` `{path:line}` `{cwd}` `{pane}` `{tab}`
`{window}` `{title}` `{domain}` `{host}` `{last_command}` `{last_output}` `{exit_code}`

`{path}` and `{path:line}` come from a file path under the pointer such as
`src/main.rs:12:5`. The `tab` menu has only the pane and tab values (no click values). A
placeholder a menu does not offer is a config error; one that has no value at click time (for
example `{link}` off a link) greys the item out. Write `{{` and `}}` for literal braces; braces
around anything that is not a placeholder name (`awk '{print $1}'`) are left alone.

## Safety

Text from the terminal (a selection, a link, a window title, command output) can be written by
anyone whose output reaches your screen, so microterm treats it as untrusted:

- A placeholder fills **one** argument of a `spawn` command and is never split into more. There
  is no shell in between. If you need shell syntax, read the value from the environment:
  `command = ["sh", "-c", "grep -rn -- \"$MT_SELECTION\" ."]`.
- `send` values are single-quoted for the shell by default.
- Control characters (including escape sequences) are removed, values are cut at 4 KiB, and a
  value with a newline is refused unless the item sets `multiline = true`.
- `confirm = true` always asks first and shows the exact text or command. `confirm = "auto"` asks
  when the pane is remote, when it matches `[menu_security] confirm_panes`, or when a `send` or
  `spawn` uses terminal text. Anything that matches `[menu_security] dangerous` always asks.
- Spawned programs get `MT_SELECTION`, `MT_WORD`, `MT_LINE`, `MT_LINK`, `MT_PATH`, `MT_CWD`,
  `MT_PANE`, `MT_TAB`, `MT_WINDOW`, `MT_TITLE`, `MT_DOMAIN`, `MT_MENU` and `MT_ITEM`. They can
  reach `microterm ctl` only when the item sets `spawn.ctl = true`.
- Every `send`, `spawn` and `ctl` item that runs is reported as a `menu.invoked` event
  (`microterm ctl events --type menu.invoked`).

```toml
[menu_security]
dangerous = ["rm -rf", "DROP ", "kubectl delete", "git push --force"]
confirm_panes = ["title:*prod*", "domain:*prod*"]
```

Don't put secrets in menu items; keep them in your environment or a keychain helper.

## Mistakes in the config

Menu problems show in the config banner with a line and column, and microterm keeps using the
last valid config:

```text
microterm.toml:12:15 — context_menu.viewprot: unknown menu "viewprot" (did you mean "viewport"?); menus: viewport, tab
microterm.toml:20:1 — context_menu.tab.items[0]: placeholder {selection} is not available in menu "tab"; available: {pane} {tab} …
```

`microterm ctl menu check` runs the same checks without a running instance.

## From the command line, keys and the palette

```sh
microterm ctl menu list                               # every menu, its facts and items
microterm ctl menu show viewport --pane 3             # the rows for pane 3
microterm ctl menu invoke viewport/zoom --pane 3      # run an item
microterm ctl menu invoke viewport/blame --pane 3 --dry-run --json
microterm ctl menu check
microterm ctl menu icons                              # icon names for `icon = "…"`
```

`invoke` needs a pane (`--pane`, `--match` or `$MICROTERM_PANE`); it never picks the focused
pane. It exits 3 for an unknown item, 1 when the item's `when` does not hold, and 8 when it would
ask for confirmation (add `--force`). Built-ins that need a window (Copy, Paste, Search) fail on a
`--headless` instance. `microterm sh --readonly` allows `list`, `show`, `check` and
`invoke --dry-run`.

`InvokeMenuItem` runs an item on the focused pane from a key, and configured items with an `id`
are listed in the command palette as "Viewport menu: …":

```toml
[[keys]]
key = "cmd+shift+b"
action = { InvokeMenuItem = "viewport/blame" }
```
