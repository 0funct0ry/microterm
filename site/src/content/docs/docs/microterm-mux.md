---
title: microterm-mux
description: The headless daemon that keeps remote terminals alive.
---

`microterm-mux` is a small, statically linked program with no GUI dependencies. microterm
installs it on a host the first time you open an [SSH domain](/docs/ssh-domains/) there; you can
also use it by hand.

```text
microterm-mux serve [--foreground]   run the per-user daemon (detaches unless --foreground)
microterm-mux proxy                  bridge stdin/stdout to the daemon, starting it if needed
microterm-mux list                   panes, sessions, size, state, title
microterm-mux kill PANE_ID           end a pane
microterm-mux kill --session NAME    end every pane of a session
microterm-mux --version
```

## How it works

- One daemon per user per host. `serve` takes a lock, binds the socket, then detaches with
  `setsid` and a double fork. Its log is `daemon.log` next to the socket.
- The socket is `$XDG_RUNTIME_DIR/microterm-mux/sock`, or `/tmp/microterm-mux-$UID/sock`, in a
  `0700` directory. Connections from other users are refused (`SO_PEERCRED` / `getpeereid`).
- The daemon owns the PTYs and keeps an authoritative terminal for each pane, so a pane survives
  any number of client disconnects, indefinitely. Panes are grouped in *sessions*; microterm uses
  one session per domain, named after it.
- microterm talks to it through `microterm-mux proxy` on an SSH exec channel, using
  length-prefixed binary frames with a version check in the first message.

## Versions

The binary is installed as `microterm-mux-<version>` under `<remote_dir>/bin`, so different
microterm versions can live side by side. `list` and `kill` talk to the running daemon of the
version you invoke.

## Static builds

```sh
make mux          # for this machine (debug); make mux-release for an optimised build
make mux-musl     # static Linux binaries (x86_64 and aarch64) for remote hosts
```

`make mux-musl` adds the Rust targets itself. Natively on Linux it needs nothing else; from
macOS or another architecture it cross-builds with `cargo-zigbuild` (installed on demand) and
needs [`zig`](https://ziglang.org) on your `PATH`.

`make lint` checks that `mt_mux_server` does not depend on `gpui` or `tokio`.
