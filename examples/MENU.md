# Context menu examples

Twenty additions for `~/.config/microterm/microterm.toml`. Each snippet stands on its own; copy the
ones you want. Every snippet passes `microterm ctl menu check`. The full grammar (item kinds,
`when` keys, placeholders, safety rules) is in the
[Context menus](../site/src/content/docs/docs/context-menus.md) docs.

A few things to know before copying:

- Placeholders such as `{selection}` fill **one** argument of a `spawn` command and are never split
  or run through a shell. When you need shell syntax, use `sh -c` and read the value from the
  environment (`$MT_SELECTION`, `$MT_WORD`, `$MT_PATH`, …) as several examples below do.
- `send` values are single-quoted for the shell unless the item sets `quote = "none"`.
- A comma separates `when` clauses, so a regex in `when` cannot contain a literal `,` (write `\x2c`).
- Clipboard examples use `pbcopy` (macOS). On Linux use `wl-copy` (Wayland) or `xclip -selection clipboard`.
- Items with an `id` also appear in the command palette ("Viewport menu: …") and can be bound to a
  key with `action = { InvokeMenuItem = "viewport/<id>" }`.

## Acting on the selection or the word under the pointer

### 1. Search git history for the selected text

```toml
[[context_menu.viewport.items]]
id    = "git_log_s"
label = "git log -S selection"
icon  = "search"
when  = "selection"
spawn = { command = ["git", "-C", "{cwd}", "log", "-S", "{selection}", "--oneline"], where = "split-right" }
```

### 2. Grep the current directory for the selection

```toml
[[context_menu.viewport.items]]
id    = "rg_selection"
label = "Search here for selection"
icon  = "regex"
when  = "selection, local"
spawn = { command = ["rg", "--heading", "--line-number", "--", "{selection}"], cwd = "{cwd}", where = "split-down" }
```

### 3. Open the manual page for the word under the pointer

```toml
[[context_menu.viewport.items]]
id    = "man_word"
label = "man {word}"
icon  = "info"
when  = "word:re:^[a-z][a-z0-9_.-]*$"
spawn = { command = ["man", "{word}"], where = "split-right", close_on_success = true }
```

### 4. Pretty-print the selected JSON in a split

```toml
[[context_menu.viewport.items]]
id    = "jq_selection"
label = "Pretty-print selected JSON"
when  = "selection:re:^\\s*[\\[{]"
spawn = { command = ["sh", "-c", "printf '%s' \"$MT_SELECTION\" | jq . ; exec sh"], where = "split-right" }
```

### 5. Decode the selected base64

```toml
[[context_menu.viewport.items]]
id    = "base64_decode"
label = "Decode base64"
when  = "selection:re:^[A-Za-z0-9+/=]+$"
spawn = { command = ["sh", "-c", "printf '%s' \"$MT_SELECTION\" | base64 -d; echo; exec sh"], where = "split-down" }
```

## Paths, links and hosts

### 6. Open `file:line` in your editor

`{path:line}` comes from a path under the pointer such as `src/main.rs:42:7`. Without one the
item is greyed out.

```toml
[[context_menu.viewport.items]]
id    = "open_in_editor"
label = "Open {path:line} in VS Code"
icon  = "folder"
spawn = { command = ["code", "-g", "{path:line}"], cwd = "{cwd}", close_on_success = true }
```

### 7. Follow a log file under the pointer

```toml
[[context_menu.viewport.items]]
id    = "tail_path"
label = "tail -f {path}"
spawn = { command = ["tail", "-n", "200", "-f", "{path}"], cwd = "{cwd}", where = "split-down" }
```

### 8. Copy a link as a Markdown link

```toml
[[context_menu.viewport.items]]
id    = "copy_md_link"
label = "Copy link as Markdown"
icon  = "copy"
when  = "link"
spawn = { command = ["sh", "-c", "printf '[%s](%s)' \"${MT_SELECTION:-$MT_LINK}\" \"$MT_LINK\" | pbcopy"], close_on_success = true }
```

### 9. SSH to the host under the pointer

Asks first, because the host name comes from terminal output.

```toml
[[context_menu.viewport.items]]
id      = "ssh_word"
label   = "ssh {word}"
icon    = "terminal"
when    = "word:re:^[A-Za-z0-9][A-Za-z0-9.-]*\\.[A-Za-z]+$"
confirm = "auto"
spawn   = { command = ["ssh", "{word}"], where = "tab", title = "ssh {word}" }
```

## Shell integration (needs `eval "$(microterm shell-integration zsh)"` or the bash/fish snippet)

### 10. Re-run the last command

`quote = "none"` sends the command line as it was typed. `confirm = "auto"` shows it before it runs.

```toml
[[context_menu.viewport.items]]
id      = "rerun"
label   = "Re-run: {last_command}"
icon    = "refresh-cw"
when    = "idle, !alt_screen"
send    = "{last_command}\n"
quote   = "none"
confirm = "auto"
```

### 11. Copy the last command's output

The script reads the output back through `microterm ctl`, so the item sets `spawn.ctl = true`.

```toml
[[context_menu.viewport.items]]
id    = "copy_last_output"
label = "Copy last command output"
icon  = "copy"
when  = "idle"
spawn = { command = ["sh", "-c", "microterm ctl pane get-text --pane \"$MT_PANE\" --last-command | pbcopy"], ctl = true, close_on_success = true }
```

### 12. Ask an agent to explain the last failure

```toml
[[context_menu.viewport.items]]
id    = "explain_failure"
label = "Explain the last error"
icon  = "triangle-alert"
when  = "last_exit!=0, idle"
spawn = { command = ["sh", "-c", "microterm ctl pane get-text --pane \"$MT_PANE\" --last-command | claude -p 'Explain this failure and suggest a fix'; exec sh"], ctl = true, where = "split-down" }
```

## DevOps

### 13. Describe the Kubernetes pod under the pointer

`send` shell-quotes `{word}`. On panes whose title or domain matches `*prod*` (the default
`[menu_security] confirm_panes`), and whenever terminal text is used, `confirm = "auto"` asks first.

```toml
[[context_menu.viewport.items]]
id      = "kubectl_describe"
label   = "kubectl describe pod {word}"
when    = "word:re:^[a-z0-9]([-a-z0-9]*[a-z0-9])?$, !alt_screen"
send    = "kubectl describe pod {word}\n"
confirm = "auto"
```

### 14. Follow a Docker container's logs

```toml
[[context_menu.viewport.items]]
id    = "docker_logs"
label = "docker logs -f {word}"
when  = "word:re:^[0-9a-f]{12}[0-9a-f]*$"
spawn = { command = ["docker", "logs", "-f", "--tail", "100", "{word}"], where = "split-right", title = "logs {word}" }
```

### 15. Show the commit under the pointer

```toml
[[context_menu.viewport.items]]
id    = "git_show"
label = "git show {word}"
when  = "word:re:^[0-9a-f]{7}[0-9a-f]*$, local"
spawn = { command = ["git", "-C", "{cwd}", "show", "--stat", "-p", "{word}"], where = "split-right" }
```

## Layout, status and organisation

### 16. A "Git" submenu with a header

```toml
[[context_menu.viewport.items]]
id    = "git_header"
type  = "header"
label = "Git"

[[context_menu.viewport.items]]
id    = "git"
label = "Git"
when  = "idle, !alt_screen"
submenu = [
  { id = "git_status", label = "Status", send = "git status\n" },
  { id = "git_diff",   label = "Diff",   send = "git diff --stat\n" },
  { id = "git_log",    label = "Log",    send = "git log --oneline -n 20\n" },
  { type = "separator" },
  { id = "git_lazy",   label = "Open lazygit", spawn = { command = ["lazygit"], cwd = "{cwd}", where = "tab", close_on_success = true } },
]
```

### 17. Run a monitor next to the pane

```toml
[[context_menu.viewport.items]]
id    = "htop_split"
label = "htop below"
icon  = "maximize-2"
spawn = { command = ["htop"], where = "split-down", close_on_success = true }
```

### 18. Mark the pane as waiting for review (agents and humans)

A `ctl` item runs `microterm ctl` as the clicked pane and shows the result as a toast.

```toml
[[context_menu.viewport.items]]
id    = "needs_review"
label = "Mark as needs review"
icon  = "triangle-alert"
when  = "status:idle"
ctl   = "pane status set --state waiting 'needs review'"

[[context_menu.viewport.items]]
id    = "clear_status"
label = "Clear pane status"
when  = "!status:idle"
ctl   = "pane status clear"
```

## The tab menu

### 19. Duplicate a tab, or open its directory in a new window

The tab menu acts on the tab's active pane; it has no selection or link values.

```toml
[[context_menu.tab.items]]
id    = "duplicate_tab"
label = "Duplicate tab"
icon  = "plus"
spawn = { command = ["sh", "-c", "exec \"${SHELL:-sh}\" -l"], cwd = "{cwd}", where = "tab" }

[[context_menu.tab.items]]
id    = "new_window_here"
label = "New window here"
spawn = { command = ["sh", "-c", "exec \"${SHELL:-sh}\" -l"], cwd = "{cwd}", where = "window" }
```

### 20. One layout group, shared by the viewport and tab menus

Groups are checked against each menu that includes them, so they may only use what both menus
offer.

```toml
[[menu_groups.layout.items]]
type  = "header"
label = "Layout"

[[menu_groups.layout.items]]
id     = "layout_zoom"
label  = "Zoom"
icon   = "maximize-2"
action = "TogglePaneZoom"

[[menu_groups.layout.items]]
id     = "layout_rotate"
label  = "Rotate"
when   = "panes>1"
action = "RotatePanes"

[[menu_groups.layout.items]]
id     = "layout_split_left"
label  = "Split left"
action = { Split = "left" }

[[context_menu.viewport.items]]
include = "layout"

[[context_menu.tab.items]]
include = "layout"
```
