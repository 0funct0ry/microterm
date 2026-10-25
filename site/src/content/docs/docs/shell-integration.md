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

## Pane keys and palette entries: `mt-key`, `mt-palette`

```sh
mt-key add [--id ID] [--ctl] CHORD -- COMMAND [ARG...]
mt-palette add [--id ID] [--ctl] LABEL -- COMMAND [ARG...]
mt-key clear [ID]    mt-palette clear [ID]
```

They register a key chord or a "From pane" command-palette entry that works only in this pane
(`OSC 7723` / `OSC 7724`; see [Private OSC](/docs/private-osc/#7723-pane-key-bindings-and-7724-palette-entries)).
The command is quoted for your shell and typed into the pane; `--ctl` runs `microterm ctl COMMAND`
for this pane. They never override your own or a built-in key, and are inert until
`[private_osc] enabled = true`.

## Menu items: `mt-menu`

The snippets also define `mt-menu`, which adds items to the right-click menu of the current pane
(see [Context menus](/docs/context-menus/#items-registered-by-the-shell) for what they may do):

```sh
mt-menu add [--id ID] [--ctl|--spawn] LABEL -- COMMAND [ARG...]
mt-menu clear [ID]
```

The default kind types the quoted command into the pane and presses Enter. `--ctl` runs
`microterm ctl COMMAND` for this pane, and `--spawn` opens the command in a split (trusted
directories only). `mt-menu` writes a private escape sequence, `OSC 7717 ; menu ; add|clear ; …`,
so it also works over ssh and inside tmux-less remote shells; anything that can print to the
terminal can register items, which is why they are restricted. `OSC 7717` is part of the
[private OSC](/docs/private-osc/) range: it is **inert until `[private_osc] enabled = true`** is set,
and turning that off again clears every registered item.

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
