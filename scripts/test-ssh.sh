#!/usr/bin/env bash
# SSH integration tests (crates/mt_ssh/tests/integration.rs).
#
#   scripts/test-ssh.sh            Docker openssh-server (Linux CI). Needs a *Linux* microterm-mux
#                                  in $MICROTERM_MUX_BINARY (`make mux-musl`, matching the
#                                  container's architecture).
#   scripts/test-ssh.sh --local    A throw-away sshd on 127.0.0.1 run as the current user (no root,
#                                  key auth only); uses the host's own microterm-mux build.
set -euo pipefail
cd "$(dirname "$0")/.."
WORK="$(mktemp -d)"
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  docker rm -f mt-ssh-test >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

ssh-keygen -q -t ed25519 -N '' -f "$WORK/id" -C mt-test

if [ "${1:-}" = "--local" ]; then
  cargo build -q -p mt_mux_server
  export MICROTERM_MUX_BINARY="${MICROTERM_MUX_BINARY:-$PWD/target/debug/microterm-mux}"
  ssh-keygen -q -t ed25519 -N '' -f "$WORK/host"
  cp "$WORK/id.pub" "$WORK/authorized_keys"
  chmod 600 "$WORK/authorized_keys"
  PORT=$((20000 + RANDOM % 20000))
  cat > "$WORK/sshd_config" <<CFG
Port $PORT
ListenAddress 127.0.0.1
HostKey $WORK/host
AuthorizedKeysFile $WORK/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
PidFile $WORK/sshd.pid
Subsystem sftp internal-sftp
CFG
  /usr/sbin/sshd -D -e -f "$WORK/sshd_config" 2>"$WORK/sshd.log" &
  PIDS+=($!)
  # Lets the tests kill the server side of a connection (its sshd-session processes).
  export MT_SSH_TEST_SSHD_PID=$!
  for _ in $(seq 50); do (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break; sleep 0.1; done
  export MT_SSH_TEST_HOST=127.0.0.1 MT_SSH_TEST_PORT=$PORT MT_SSH_TEST_USER="$(id -un)"
else
  : "${MICROTERM_MUX_BINARY:?set MICROTERM_MUX_BINARY to a Linux microterm-mux build}"
  export MICROTERM_MUX_BINARY
  PORT=2222
  docker run -d --name mt-ssh-test -p $PORT:2222 \
    -e USER_NAME=tester -e PASSWORD_ACCESS=true -e USER_PASSWORD=tester-pw \
    -e PUBLIC_KEY="$(cat "$WORK/id.pub")" lscr.io/linuxserver/openssh-server >/dev/null
  for _ in $(seq 100); do (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break; sleep 0.2; done
  sleep 2
  export MT_SSH_TEST_HOST=127.0.0.1 MT_SSH_TEST_PORT=$PORT MT_SSH_TEST_USER=tester MT_SSH_TEST_PASSWORD=tester-pw
fi
export MT_SSH_TEST_KEY="$WORK/id"
cargo test -p mt_ssh --test integration -- --nocapture --test-threads=1
