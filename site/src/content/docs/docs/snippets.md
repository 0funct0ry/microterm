---
title: Snippets
description: Keep reusable commands in snippets.toml and insert them into a pane from a side panel, the palette, a key or the command line. Optional {{ }} templates fill in values, system facts and prompts.
---

Snippets are saved pieces of text: commands, one-liners, boilerplate. You keep them in
`snippets.toml` and insert them into the focused pane from a **side panel**, the command palette,
a key binding, `microterm ctl` or an agent. With templates on, a body such as `echo {{uuid}}` or
`git commit -m {{ input "Message" | quote }}` expands when you insert it.

Both the feature and template expansion are **off by default**.

## Turning it on

```toml
# microterm.toml
[snippets]
enabled = true      # the panel, palette provider, keys and `ctl snippet`
templates = true    # expand {{ }} tags; false inserts bodies exactly as written
```

Neither switch needs a restart. With `enabled = false` (the default) nothing is read or watched,
no panel exists, `⌘⇧S` only shows "Snippets are disabled" with an **Open config** button, and
`microterm ctl snippet …` exits `10`. `templates = true` without `enabled` has no effect (an info
note in the config diagnostics). Turning `templates` on changes the next insert; turning
`enabled` off closes the panel and any open dialog.

| `enabled` | `templates` | Behaviour |
|---|---|---|
| `false` | any | inert |
| `true` | `false` | snippets work; `{{uuid}}` is inserted exactly as written, no dialog |
| `true` | `true` | full behaviour below |

The other `[snippets]` keys are `panel_width` (220-600 px), `panel_open_on_start`,
`confirm_multiline`, `focus_terminal_after_insert` and `preview_lines` (0-5); see the
[config file](/docs/config-file/#snippets).

## snippets.toml

It lives next to `microterm.toml`. A missing file is an empty set; the panel creates it on your
first save.

```toml
[[snippets]]
id = "k8s-logs"                  # optional; see below
title = "Tail pod logs"          # required, one line
body = '''kubectl -n {{ choice "Namespace" "dev" "staging" "prod" }} logs -f {{ input "Pod" }}'''
folder = "k8s"                   # optional group header in the panel
tags = ["kubectl", "logs"]       # optional, searchable
description = "Follow the logs of one pod"
run = false                      # press Enter after inserting
keys = "cmd+alt+1"               # optional key binding
```

- **`id`** is `a-z`, `0-9`, `_` and `-` (at most 64 characters). Without one it is derived from the
  title (`tail-pod-logs`, then `-2`, `-3` on collisions); saving from the panel always writes it.
  A duplicate explicit id skips that entry and shows a banner.
- A body is required, at most 64 KiB; a file holds at most 5,000 snippets. File order is the
  display order, and a folder appears where its first snippet is.
- **Hand edits apply live.** An invalid file keeps the last valid set and shows a banner that
  names `snippets.toml` with line and column. Unknown keys are warnings with a suggestion.
- **Saving from the panel** changes only the entries you touched. Comments, formatting and every
  other entry stay byte-identical, the previous file is kept as `snippets.toml.bak`, and the write
  is atomic. If the entry changed on disk while you were editing, Save is refused with **Reload
  and re-apply**.
- `keys` binds the snippet to a key (`[[keys]]` syntax). Precedence is registry defaults < a
  snippet's `keys` < your `[[keys]]`; replacing a default is a warning that names it. You can also
  bind it yourself:

```toml
[[keys]]
key = "cmd+alt+2"
action = { InsertSnippet = { id = "k8s-logs", run = true } }
```

## The side panel

`⌘⇧S` / `Ctrl+Shift+S` (`ToggleSnippetsPanel`) cycles: closed → open with the search field
focused → (if the terminal has focus) focus the panel → close. `FocusSnippetsPanel` only focuses.
The panel sits on the right, narrows the panes and resizes their terminals; drag its left edge to
change the width (220-600 px). Open state and width are remembered in `ui.json` under
`$XDG_STATE_HOME/microterm/`.

Each row shows the title, the first body lines (template tags in the accent colour), a `↵` badge
when `run = true`, and a red badge with the first template error. Two icon buttons end the
title line: **✎** edits and **🗑** deletes (after a confirmation). The right-click menu
has the same actions and more. Typing in the search field ranks by title, body, tags, folder and description
and flattens the folders.

| Gesture | Effect |
|---|---|
| click / `Enter` | insert; the snippet's `run` decides whether Enter is pressed |
| `⌥`-click / `⌥Enter` | insert and run |
| `⇧`-click / `⇧Enter` | insert without running, even if `run = true` |
| `⌘`-click / `⌘Enter` (`Ctrl` on Linux) | insert into every pane of the tab |
| `⌘E` / `Ctrl+E` | edit |
| right-click | Insert, Insert & Run, Edit, Duplicate, Copy expanded, Copy raw, Move to folder…, Delete… |
| drag a row | reorder within its folder (written back) |
| `↑` `↓`, `Esc` | move; `Esc` clears the query, then returns focus to the terminal |

A second click on the same row within 400 ms is ignored, so a double-click never inserts twice.

### The editor

New, Duplicate, Edit, **Save selection as snippet…** (right-click a selection) and **Save last
command as snippet** (palette; needs [shell integration](/docs/shell-integration/)) open an
editor inside the panel: title, folder, tags, description, a body editor with tag highlighting and
inline errors (`line:col`), *Run after insert*, a key, and a live preview (150 ms after your last
edit; prompts show as ‹Label›, secrets as ‹secret›, counters are not advanced). `⌘S` / `Ctrl+S`
saves; Save is disabled while the title or body is empty, the key is invalid, or, with templates
on, the body does not parse. `Esc` asks before discarding changes, and a draft survives closing
the panel.

## Templates

With `templates = true` a body can contain tags:

```text
echo {{uuid}}
git commit -m {{ input "Message" | quote }}
cd {{ git_root }} && git checkout -b {{ b := input "Branch" | kebab }} && git push -u origin {{ $b }}
curl -H "Authorization: Bearer {{ secret "Token" }}" https://api.example.com/{{ choice "Env" "dev" "prod" }}
mkdir -p backups/{{ date }} && tar czf backups/{{ date }}/{{ cwd_basename }}-{{ timestamp }}.tgz .
psql "postgres://{{ user }}@localhost/{{ input "Database" default="dev" }}"{{ cursor }}
```

- `{{ name args }}` calls a function; `| filter` pipes the value on; `\{{` is a literal `{{`.
  Whitespace inside a tag is optional. The full catalogue is in
  [Template functions](/docs/reference/template-functions/).
- **Every call site is fresh**: two `{{uuid}}` give two uuids. `{{ id := uuid }}` binds the value
  and emits it, `{{ $id }}` emits it again, and `{{ id := uuid | silent }}` binds without emitting.
- `{{ git_branch default="main" }}`: any function accepts `default=` to replace a runtime failure.
- Mistakes (unknown function, wrong arguments, an unbound `$name`, a second `cursor`) are caught
  when the snippet loads: the panel shows a red badge, the editor shows `line:col`, and a snippet
  with errors cannot be inserted.
- Functions only read what is on screen or on this machine: the clock, a CSPRNG, the pane's
  state, `.git/HEAD`, the clipboard. **None runs a program or touches the network.**

### Prompts: the fill dialog

`input`, `choice` and `secret` open the **fill dialog** before inserting: one field per label (a
label used twice is asked once), a live preview of exactly what will be inserted, and these keys:

| Key | Does |
|---|---|
| `Tab` / `Shift+Tab` | next / previous field |
| `Enter` | insert |
| `⌥Enter` / `Alt+Enter` | insert and run |
| `Esc` | cancel; nothing is sent |

`input` is prefilled with the value you used last time in this window (or its `default=`);
`choice` is a filterable list whose first entry is preselected; `secret` is masked and shows as
`••••` in the preview. Secrets are never remembered.

## What gets sent

1. The text is cleaned like a paste: `\r\n` becomes `\n`, and NUL and ESC are removed, so
   clipboard, selection or command output cannot inject escape sequences.
2. It is **pasted**: wrapped in bracketed paste when the program asked for it, otherwise `\n`
   becomes `\r`. With `confirm_multiline = true` a multi-line insert asks first.
3. If the snippet runs, Enter (`\r`) follows.
4. If the template has `{{cursor}}`, the text is a single line and the snippet does not run, the
   shell cursor is moved left to the marker (`←` presses, or the application-cursor form in
   programs that asked for it; at most 1,000).
5. Focus returns to the terminal (`focus_terminal_after_insert`), and `counter` values advance. A
   failure (for example `git_branch` in a remote pane) aborts the insert, sends nothing and shows
   a toast naming the tag.

## Other ways in

- **Palette**: `!` lists snippets ("Snippet: Tail pod logs", with the folder and first line);
  `Enter` inserts, `⌥Enter` runs. The snippet actions (`NewSnippet`, `OpenSnippetsFile`, …) are
  listed under "Snippets" while the feature is on.
- **Keys**: `keys` in a snippet, or `InsertSnippet` in `[[keys]]` (see above).
- **Command line**: `microterm ctl snippet list|show|expand|insert|add|rm`, from
  [`microterm sh`](/docs/sh/) too; see [`ctl snippet`](/docs/ctl/#snippet).
- **Agents**: the MCP server adds `microterm_snippet_list`, `microterm_snippet_expand` and
  `microterm_snippet_insert` while snippets are enabled; see
  [Agents & automation](/docs/agents-and-automation/).

## Security and privacy

- System functions (`user`, `hostname`, `os`, `home`, `local_ip`, …) always describe **this**
  machine, even in an SSH pane. `env NAME` reads microterm's own environment.
- An inserted **secret reaches the pane like typed input**: it is visible in that pane's
  scrollback and in `--input` recordings, as a typed password is. It is never logged, kept in the
  palette history, remembered between inserts, shown in a preview, printed by `ctl` or passed
  through MCP (an agent request for a snippet with a `secret` prompt is refused).
- `ctl snippet insert` is an input verb like `pane send-text`: it needs an explicit target and
  never falls back to the focused pane. Pass secrets with `--vars-file -` (stdin), not `--var`,
  because argv is visible to other processes. `microterm sh` does not keep `snippet insert`
  lines in its history unless `[sh] history_send_text = true`.

## Not in this version

Functions that run a program or make a request, loops and conditionals in templates, snippet
sharing or sync, per-workspace or per-project snippet files, nested folders, a left-hand panel,
evaluating system functions on the remote host, Lua-defined functions and importing from other
tools.
