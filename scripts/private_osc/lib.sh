# Shared helpers for the private OSC demo scripts (source this file, do not run it).
# Run the scripts inside a microterm pane with `[private_osc] enabled = true`.

b64() { printf %s "$1" | base64 | tr -d '\n'; }
unb64() { base64 -d 2>/dev/null || base64 -D 2>/dev/null; }
step() { printf '\n== %s\n' "$*"; }
pause() { printf '   (press Enter) '; read -r _ </dev/tty; }

# status_set SLOT TEXT [LEVEL] [TOOLTIP]   (7725)
status_set() { printf '\e]7725;status;set;%s;%s;%s;%s\a' "$1" "$(b64 "$2")" "${3:-info}" "$(b64 "${4:-}")"; }
# status_clear [SLOT]
status_clear() { printf '\e]7725;status;clear%s\a' "${1:+;$1}"; }

require_pane() {
  [ -t 1 ] || { echo "run this in a microterm pane" >&2; exit 1; }
}
