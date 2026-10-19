---
title: CLI overview
description: Script, automate and record a running microterm from the shell.
---

`microterm` is also a command-line client for the instance that is already running. Every GUI
process listens on a Unix socket; `microterm ctl …` talks to it over a small JSON-lines protocol,
so scripts can open tabs, type into them, read their output, close them and record them.
`ctl` is organised by resource: `workspace`, `window`, `tab` and `pane`.

```sh
microterm ctl workspace create build
eval "$(microterm ctl tab new --workspace build -- cargo watch -x test)"   # sets $tab $pane $window
microterm ctl pane send-text --pane "$pane" 'echo hello'
microterm ctl pane get-text --pane "$pane"
microterm ctl tab close --tab "$tab" --force
```

## Which instance?

1. `$MICROTERM_UNIX_SOCKET`, if set. Every pane microterm starts has it set, so commands run
   inside a tab always reach the instance that owns the tab.
2. Otherwise the newest live socket in the socket directory: `$XDG_RUNTIME_DIR/microterm/` on
   Linux (`/tmp/microterm-<uid>/` without `$XDG_RUNTIME_DIR`), and
   `~/Library/Application Support/microterm/sock/` on macOS. Sockets left by a crashed instance
   are removed when they are found dead.

With no instance running every `ctl` command exits with code **2**. The directory is created
with mode `0700` and the server refuses connections from other users.

## Which pane, tab or window?

Name it with `--pane|--tab|--window ID` or `--match EXPR`. With neither, a command run inside a
pane targets that pane, tab or window through `$MICROTERM_PANE`, `$MICROTERM_TAB` and
`$MICROTERM_WINDOW`, so a script can write `microterm ctl pane get-text` and mean its own pane.
Read-only verbs fall back to the focused item; input and destructive verbs never do. The full
rules and the selector language are in [Selectors & targeting](/docs/selectors-and-targeting/).
`microterm ctl list` shows the ids.

## Output

Commands print plain text for people (tab-separated rows when piped) and, with `--json`, one
JSON value for scripts. Errors go to stderr, as a JSON object with `--json`. See [Exit codes](/docs/reference/exit-codes/).

## Other commands

| Command | Does |
|---------|------|
| `microterm completions bash\|zsh\|fish` | prints a shell completion script |
| `microterm record -o FILE.cast -- CMD` | runs CMD in a new tab and records it |
| `microterm shell-integration bash\|zsh\|fish` | prints the shell snippet that emits OSC 133 marks (see [Shell integration](/docs/shell-integration/)) |
| `microterm --workspace NAME`, `--cwd`, `-e CMD` | start the GUI, or open a window in the running instance |

See [`microterm ctl`](/docs/ctl/) for every command, [Run & wait](/docs/run-and-wait/) for `pane run`
and `pane wait`, and [Recording & replay](/docs/recording-and-replay/).

## Shell completions

```sh
microterm completions zsh > ~/.zfunc/_microterm        # zsh
microterm completions bash > ~/.local/share/bash-completion/completions/microterm
microterm completions fish > ~/.config/fish/completions/microterm.fish
```
