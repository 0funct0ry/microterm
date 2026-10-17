#!/usr/bin/env bash
# Starts a throw-away sshd on 127.0.0.1 (as the current user, key auth only, no root) and prints
# everything you need for microterm: the [[ssh_domains]] block for microterm.toml, the daemon
# environment variable, and the known_hosts / cleanup notes. sshd then runs in the foreground;
# Ctrl-C stops it. Keys persist in ~/.cache/microterm-demo so reruns keep the same host key.
#
#   scripts/ssh-demo.sh [PORT] [DOMAIN_NAME]        defaults: 2299 demo
#   scripts/ssh-demo.sh --drop                      kill the live connections (the listener
#                                                   keeps running, so microterm reconnects)
set -euo pipefail
cd "$(dirname "$0")/.."

K="${XDG_CACHE_HOME:-$HOME/.cache}/microterm-demo"

# --drop simulates a dropped connection. Killing the listening sshd would not drop anything:
# established connections are served by separate per-connection processes and live on.
if [ "${1:-}" = "--drop" ]; then
  [ -f "$K/port" ] || { echo "error: the demo sshd was never started (make ssh-demo)" >&2; exit 1; }
  PORT="$(cat "$K/port")"
  # The server side of each established connection on the demo port: the per-connection sshd
  # processes (`sshd-session: user …` on OpenSSH 9.8+, `sshd: user …` before; a privileged
  # monitor and its child share the socket). Found by port, so this works whether or not the
  # listening sshd is still running.
  victims=""
  for p in $(lsof -nP -t -iTCP:"$PORT" -sTCP:ESTABLISHED 2>/dev/null | sort -u); do
    ps -o command= -p "$p" 2>/dev/null | grep -Eq '^sshd(-session)?: ' && victims="$victims $p"
  done
  [ -n "$victims" ] || { echo "no live connections on port $PORT to drop"; exit 0; }
  for p in $victims; do
    echo "killing $(ps -o pid=,command= -p "$p")"
    kill -9 "$p" 2>/dev/null || true
  done
  if lsof -nP -t -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "dropped. The demo sshd is still listening, so microterm reconnects within a few seconds."
  else
    echo "dropped. Nothing is listening: microterm keeps retrying until you run make ssh-demo again."
  fi
  exit 0
fi

PORT="${1:-2299}"
NAME="${2:-demo}"
SSHD=/usr/sbin/sshd
[ -x "$SSHD" ] || SSHD="$(command -v sshd || true)"
[ -n "$SSHD" ] || { echo "error: sshd not found" >&2; exit 1; }
command -v ssh-keygen >/dev/null || { echo "error: ssh-keygen not found" >&2; exit 1; }

# Keys live in a stable directory so the host key (and the identity_file path in the printed
# config) are the same on every run. A fresh host key per run would make microterm refuse to
# connect: it checks ~/.ssh/known_hosts, and a changed key for the same host:port is a hard error.
mkdir -p "$K"
chmod 700 "$K"
[ -f "$K/id" ] || ssh-keygen -q -t ed25519 -N '' -f "$K/id" -C microterm-demo
if [ ! -f "$K/host" ]; then
  ssh-keygen -q -t ed25519 -N '' -f "$K/host"
  # New host key: forget any older key recorded for this demo port (from earlier versions).
  ssh-keygen -R "[127.0.0.1]:$PORT" >/dev/null 2>&1 || true
fi
echo "$PORT" > "$K/port"
cp "$K/id.pub" "$K/authorized_keys"
chmod 600 "$K/authorized_keys"

D="$(mktemp -d)"
trap 'rm -rf "$D"; echo; echo "sshd stopped."' EXIT
cat > "$D/sshd_config" <<CFG
Port $PORT
ListenAddress 127.0.0.1
HostKey $K/host
AuthorizedKeysFile $K/authorized_keys
PasswordAuthentication no
UsePAM no
StrictModes no
PidFile $D/sshd.pid
Subsystem sftp internal-sftp
CFG

MUX="$PWD/target/debug/microterm-mux"
FP="$(ssh-keygen -lf "$K/host.pub" | awk '{print $2}')"

cat <<INFO
================ microterm demo sshd ================
host:        127.0.0.1
port:        $PORT
user:        $(id -un)
client key:  $K/id
host key:    $K/host  ($FP)
known_hosts: ~/.ssh/known_hosts gets "[127.0.0.1]:$PORT" after you trust the key

--- add to ~/.config/microterm/microterm.toml (back the file up first) ---
[[ssh_domains]]
name = "$NAME"
host = "127.0.0.1"
port = $PORT
user = "$(id -un)"
identity_file = "$K/id"

--- daemon binary (build it with: make mux) ---
export MICROTERM_MUX_BINARY=$MUX
$( [ -x "$MUX" ] && echo "(found)" || echo "(NOT BUILT YET: run make mux)" )

--- then ---
make run   # Cmd/Ctrl+Shift+L -> "New tab in $NAME", trust the fingerprint above

--- test reconnection (in another terminal) ---
make ssh-demo-drop                   # drop it; microterm reconnects by itself
Ctrl-C here, then make ssh-demo-drop  # keep it down: attempts count up until you restart this

--- cleanup afterwards ---
ssh-keygen -R "[127.0.0.1]:$PORT"; rm -rf "$K"   # forget the demo host key and keys
restore your microterm.toml; the remote daemon lives in ~/.local/share/microterm/bin/
=====================================================
sshd running in the foreground (Ctrl-C to stop)...
INFO

exec "$SSHD" -D -e -f "$D/sshd_config"
