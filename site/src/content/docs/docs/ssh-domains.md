---
title: SSH domains
description: Open tabs on remote hosts that survive dropped connections.
---

An **SSH domain** is a place where tabs run: `local` is one, and every `[[ssh_domains]]` entry
is another. A tab in an SSH domain looks and behaves like any other tab, with one difference
that matters: **its terminal lives on the remote host**, in a small daemon called
[`microterm-mux`](/docs/microterm-mux/). When the connection drops, the process keeps running.
When microterm reconnects, the same screen is back.

## Configure a domain

```toml
[[ssh_domains]]
name = "prod"
host = "prod.example.com"          # also resolved through ~/.ssh/config
user = "bruce"                     # optional
port = 22                          # optional
identity_file = "~/.ssh/id_ed25519" # optional
multiplexing = "microterm"         # "microterm" (persistent) | "none" (plain session)
remote_dir = "~/.local/share/microterm"
connect_timeout_s = 10
```

Explicit fields win over `~/.ssh/config`, which wins over defaults. Domains are read when a
tab first opens in them; editing a domain does not disturb tabs that are already connected
(see [Hot reload](/docs/hot-reload/)).

## Open a tab

- **Launcher** (`⌘⇧L` / `Ctrl+Shift+L`): *New tab in prod*. The dot shows the connection state:
  green connected, amber connecting or reconnecting, red disconnected, grey not used yet.
  *Connect to host…* asks for `[user@]host[:port]` and opens a temporary domain.
- **Command line:** `microterm ssh prod`, `microterm ssh bruce@example.com:2222`,
  `microterm ssh --domain prod`, or `microterm ctl tab new --domain prod`.
- **Keybinding / config:** the `SpawnTabInDomain` and `ConnectSsh` actions.

The title bar shows the active tab's domain with the same dot.

## What microterm reads from `~/.ssh/config`

`Host` patterns (`*`, `?`, `!negation`; the first value wins, as in OpenSSH), `HostName`, `User`,
`Port`, `IdentityFile`, `IdentitiesOnly`, `ProxyJump` (several hops, tunnelled through
`direct-tcpip` channels), `ProxyCommand` (`%h`, `%p`, `%r`) and `ServerAliveInterval`.
`Include` is not followed and `Match` blocks are ignored.

## Authentication

In order: the SSH agent (`SSH_AUTH_SOCK`), identity files (a passphrase prompt appears for
encrypted keys), keyboard-interactive, password. With `IdentitiesOnly yes` the agent is skipped
and only the configured identity files are used. Passwords and passphrases are asked for in a
modal, kept only in memory while they are used, and never written to disk or logs.

## Host keys

Keys are checked against `~/.ssh/known_hosts` (plain and hashed entries).

- **Unknown host:** a dialog shows the SHA256 fingerprint. *Trust and connect* appends it to
  `known_hosts`; *Cancel* (or `Esc`) stops the connection.
- **Changed key:** the connection fails with an explanation, and there is no override in the UI.
  Fix `known_hosts` yourself once you know why the key changed.

## First connection: installing the daemon

After you authenticate, microterm looks for `<remote_dir>/bin/microterm-mux-<version>`. If it
is missing, it detects the platform with `uname -sm`, takes the matching static binary from
`~/.cache/microterm/mux/<version>/<target>/` (downloaded once from the GitHub release and
verified against its `SHA256SUMS`), uploads it over SFTP, and moves it into place atomically.

Supported hosts: Linux and macOS on x86_64 and aarch64.

If that fails (no SFTP, read-only home, unsupported architecture), the tab falls back to a plain
SSH session with a notice explaining why. A plain session works, but **cannot survive a
disconnect**. You can ask for it explicitly with `multiplexing = "none"`.

For development, `MICROTERM_MUX_BINARY=/path/to/microterm-mux` uploads that file instead of
downloading a release.

## Reconnection

While the connection is down the tab shows a card over the dimmed last screen:

> Connection to prod lost. Reconnecting in 4 s (attempt 3).

Keyboard input is blocked. Retries wait 1, 2, 4, 8, 16, then 30 seconds (±20 % jitter).
Three missed keepalives (`ServerAliveInterval`, default 10 s) or a read error start it.

- **Reconnect now** (or `Enter`) skips the wait.
- **Close tab** closes the tab here; the remote process keeps running on the host.

On reconnect the daemon sends the screen and scrollback again as a synthesized terminal stream,
then live output continues. A pane whose process ended while you were away shows
`[process exited with status N]`. Failures that waiting cannot fix (changed host key,
authentication failure) stop retrying and say why; press *Retry* once you have fixed it.

Closing a tab while connected ends its remote process. To keep it, let the connection drop
first or detach with *Close tab* on the reconnect card.

## Limits

- Replay restores the screen, scrollback, modes, cursor, colours and title. It does not restore
  the scroll region, saved cursor, character sets, tab stops or hyperlinks, and the normal screen
  hidden behind a full-screen app. Full-screen apps redraw on the next resize.
- If a client falls too far behind, the daemon disconnects it and it re-attaches with a replay.
- Windows hosts are not supported.
