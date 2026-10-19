---
title: Run & wait
description: Run a command, wait for it or for output, and read exactly what it printed, without sleeps or screen scraping.
---

`microterm ctl pane run` and `pane wait` let a script (or an agent) run a command, wait until it
is ready or finished, and read its result. `pane get-text --last-command` reads the output of
the last command from a shell pane. They share the [targeting rules](/docs/selectors-and-targeting/)
of every `ctl` command and the [exit codes](/docs/reference/exit-codes/) below.

```text
microterm ctl pane run  [--window ID | --new-window] [--domain NAME] [--cwd DIR] [--title T]
                        [--keep] [--detach] [--timeout DUR] [--propagate-exit] -- CMD ARGS…
microterm ctl pane run  --in TARGET [--timeout DUR] -- COMMAND LINE…
microterm ctl pane wait [TARGET] (--match REGEX | --idle DUR | --exit | --command-finished)…
                        [--timeout DUR] [--new-only]
microterm ctl pane get-text [TARGET] --last-command [N]
```

Durations are written `500ms`, `2s`, `5m` (or `1h`); a bare number is seconds.

## pane run: spawn mode

```sh
microterm ctl pane run --json -- cargo test
```

Opens a new tab that runs `CMD ARGS…` **directly** (an argument vector, no shell parsing; use
`-- sh -c '…'` for pipes and `&&`) and blocks until the process exits. The result:

```json
{"pane":14,"tab":9,"exit_code":101,"signal":null,"duration_ms":4120,"output":"…","truncated":false}
```

- `output` is everything the process printed, rendered as text (colours and cursor movement
  applied, soft wraps joined). It is capped at `[ctl].capture_max_bytes` (1 MiB by default);
  when it is cut the **tail** is kept and `truncated` is `true`.
- The tab closes when the process exits. `--keep` leaves it open, showing
  `[process exited with status N]`; the pane then lists as `exited: true` with its `exit_code`.
- `--detach` returns `{"pane":14,"tab":9}` at once and leaves the command running. Follow it
  with `pane wait`.
- `ctl` itself exits `0` when the command ran, whatever its status. `--propagate-exit` makes it
  exit with the command's status instead. Statuses 1 to 9 collide with the codes below, so they
  become `1` (the real value stays in `--json`); a process killed by a signal exits `128 + signal`.
- `--timeout DUR` stops waiting after DUR: the partial `output` is printed with
  `"timed_out":true`, `ctl` exits **6**, and **the process keeps running**. A timeout never
  kills anything.

`--window`, `--new-window`, `--domain`, `--cwd` and `--title` place the tab like
[`tab new`](/docs/ctl/#tab). `run` never accepts `--all`.

## pane run --in: a command in an existing shell

```sh
microterm ctl pane run --in 12 --json -- git status --short
```

Types the arguments (joined with spaces; quote them yourself) and Enter into the shell of pane
12, then waits for the command to finish. TARGET is a pane id, a selector, or omitted for the
pane you are running in. The result is

```json
{"pane":12,"exit_code":0,"output":" M src/main.rs\n","truncated":false}
```

and `output` is the region between the command's `C` and `D` [shell-integration marks](/docs/shell-integration/),
**not a screen scrape**: the prompt, the echoed command line and anything printed before or
after are not in it. Without marks the pane cannot tell, so:

- exit **7** if the pane has never sent OSC 133 marks;
- exit **9** if a command is already running in it (nothing is typed);
- exit **6** on `--timeout`, with the partial output; the command keeps running.

## pane wait

```sh
microterm ctl pane wait --match title:dev --match 'ready on' --timeout 120s
microterm ctl pane wait --pane 12 --command-finished
microterm ctl pane wait --pane 12 --idle 2s
microterm ctl pane wait --pane 12 --exit --json
```

Waits until one of the conditions holds (several may be given; the first to hold wins):

| Condition | Holds when |
|-----------|-----------|
| `--match REGEX` | the regex matches the pane's rendered text: first what is on screen and in scrollback, then new output. Lines are unwrapped (soft wraps joined) and matched one at a time |
| `--new-only` | with `--match`: ignore what is already there |
| `--idle DUR` | no bytes arrived from the process for DUR (the cursor blinking does not count) |
| `--exit` | the pane's process has exited (also after its tab closed) |
| `--command-finished` | the next command finishes (OSC 133 `D`); exit 7 when the pane has no marks |

The result is `{"reason":"match|idle|exit|command_finished","waited_ms":N,"match":{"text","groups","line"}?,"exit_code":N?}`;
`match.line` counts logical lines from the oldest scrollback line. The default `--timeout` is
60 s; running out of time exits **6**. If the pane closes while waiting and `--exit` was not
requested, the command fails with exit **3**.

### Which `--match` is which

`--match` has two jobs, as it names panes everywhere else in `ctl`. A value that reads as a
pane selector (`title:dev`, `cwd:~/api,!exited:true`) chooses **the pane**; any other value is
a **regular expression**. So the line above waits in the pane titled `dev` for the text
`ready on`. To be explicit, use `--select EXPR` for the selector and `--text REGEX` for a regex
that happens to look like one (`--text 'error: failed'`), or give the pane as `--pane ID` or as
the positional TARGET. `wait` is read-only, so without a target it uses `$MICROTERM_PANE` and
then the focused pane.

## pane get-text --last-command

```sh
microterm ctl pane get-text --pane 12 --last-command --json
microterm ctl pane get-text --last-command 2        # the one before
```

Returns the output of the N-th most recent finished command (default 1), cut out with the
shell's marks:

```json
{"command":"cargo test","exit_code":101,"cwd":"/Users/me/src/api","text":"…","finished_at":"2026-05-17T12:34:56.789Z"}
```

Without `--json` only the text is printed. It cannot be combined with `--start-line`,
`--end-line` or `--escapes` (exit 1), exits **7** when the pane has no marks, and **3** when
fewer than N commands are retained. microterm keeps the last **500** commands per pane; they
also survive [SSH reconnects](/docs/ssh-domains/), because the remote daemon replays its marks.
If the output began in scrollback that has since been dropped, `truncated` is `true`.

## Disconnecting

`run` and `wait` can take a while. If the `ctl` process is interrupted (Ctrl-C, a killed
script, a closed pipe), microterm stops waiting for it. **The command it started keeps running**;
only the wait is cancelled.

## Exit codes

| Code | Meaning here |
|------|--------------|
| `0` | the call completed (for `run`: the command ran, whatever its status) |
| `1` | usage error: no condition for `wait`, `--last-command` with a line range, … |
| `3` | no such pane, or the pane went away while waiting |
| `5` | the target matched several panes |
| `6` | timeout (`wait`, `run --timeout`); the command keeps running |
| `7` | needs [shell integration](/docs/shell-integration/): no OSC 133 marks in that pane |
| `9` | `run --in` while a command is already running in that pane |
