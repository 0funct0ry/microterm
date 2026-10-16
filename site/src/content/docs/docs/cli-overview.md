---
title: CLI overview
description: Script, automate and record a running microterm from the shell.
---

`microterm` is also a command-line client for the instance that is already running. Every GUI
process listens on a Unix socket; `microterm cli …` talks to it over a small JSON-lines protocol,
so scripts can open tabs, type into them, read their output and record them.

```sh
microterm cli workspace create build
pane=$(microterm cli spawn --workspace build -- cargo watch -x test)
microterm cli send-text --pane "$pane" 'echo hello'
microterm cli get-text --pane "$pane"
```

## Which instance?

1. `$MICROTERM_UNIX_SOCKET`, if set. Every pane microterm starts has it set, so commands run
   inside a tab always reach the instance that owns the tab.
2. Otherwise the newest live socket in the socket directory: `$XDG_RUNTIME_DIR/microterm/` on
   Linux (`/tmp/microterm-<uid>/` without `$XDG_RUNTIME_DIR`), and
   `~/Library/Application Support/microterm/sock/` on macOS. Sockets left by a crashed instance
   are removed when they are found dead.

With no instance running every `cli` command exits with code **2**. The directory is created
with mode `0700` and the server refuses connections from other users.

## Which pane?

`--pane` defaults to `$MICROTERM_PANE`, which is set in every pane. A script that runs inside a
tab can therefore write `microterm cli get-text` and mean its own tab. Outside a pane you must
pass `--pane ID`; `microterm cli list` shows the ids.

## Output

Commands print plain text for people and, with `--json`, the raw result of the underlying IPC
call for scripts. Errors go to stderr. See [Exit codes](/docs/reference/exit-codes/).

## Other commands

| Command | Does |
|---------|------|
| `microterm completions bash\|zsh\|fish` | prints a shell completion script |
| `microterm record -o FILE.cast -- CMD` | runs CMD in a new tab and records it |
| `microterm --workspace NAME`, `--cwd`, `-e CMD` | start the GUI, or open a window in the running instance |

See [`microterm cli`](/docs/cli/) for every subcommand and [Recording & replay](/docs/recording-and-replay/).

## Shell completions

```sh
microterm completions zsh > ~/.zfunc/_microterm        # zsh
microterm completions bash > ~/.local/share/bash-completion/completions/microterm
microterm completions fish > ~/.config/fish/completions/microterm.fish
```
