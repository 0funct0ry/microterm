---
title: Shell integration
description: Make your shell tell microterm where prompts, commands and their output begin and end.
---

Shell integration makes a shell emit two kinds of escape sequences that microterm understands:

- **OSC 133** marks around each command: `A` prompt start, `B` start of what you type, `C`
  start of the output, and `D;status` when the command finished, with its exit status.
- **OSC 7** with the working directory.

With them microterm knows exactly where a command's output is, which powers
[`pane run --in`, `pane wait --command-finished` and `pane get-text --last-command`](/docs/run-and-wait/),
and copy mode's `[` and `]` jumps between prompts. Nothing is injected automatically: you add one
line to your shell's startup file.

```sh
# ~/.bashrc
eval "$(microterm shell-integration bash)"

# ~/.zshrc
eval "$(microterm shell-integration zsh)"

# ~/.config/fish/config.fish
microterm shell-integration fish | source
```

The snippet is loaded only in interactive shells and only once per shell, so it is safe to leave in
a shared rc file. It also works over SSH: a remote shell with the snippet sends the same marks, and the
`microterm-mux` daemon keeps them for reconnects.

## What each snippet does

| Shell | Prompt marks | `C` (output starts) | `D` and status | Working directory |
|-------|--------------|---------------------|----------------|-------------------|
| bash | wraps `PS1` (after other `PROMPT_COMMAND` hooks have rebuilt it) in `A`/`B` | `PS0` (bash ≥ 4.4), otherwise a `DEBUG` trap | first `PROMPT_COMMAND` entry, with `$?` | `PROMPT_COMMAND` |
| zsh | wraps `PS1` in `A`/`B` from the last `precmd` hook | `preexec` | `precmd`, with `$?` | `precmd` |
| fish | wraps `fish_prompt` | `fish_preexec` | `fish_postexec`, with `$status` | `--on-variable PWD` |

The snippets keep your own prompt: they wrap whatever `PS1`, `PROMPT` or `fish_prompt` contains and do not
replace `PROMPT_COMMAND` or the zsh hook arrays (bash's array form is supported).

## Checking it works

```sh
microterm ctl pane get-text --last-command --json
```

returns the previous command with its text and status. Exit code **7** means the pane has not sent
any marks: the snippet is not loaded in that shell, or it is not interactive.

## Limits

- The command text is read from the screen between `B` and `C`, so a right-aligned prompt
  (`RPROMPT`) on the input line is included in it.
- Programs that draw full-screen (vim, htop) use the alternate screen; marks are ignored there.
- microterm keeps the last 500 commands per pane. Resizing a window's **width** re-wraps the
  text, which drops the retained commands; the running command is unaffected.
