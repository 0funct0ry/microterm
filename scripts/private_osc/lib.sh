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

# progress_set ID PCT|- LABEL [ETA_S]   (7720)
progress_set() { printf '\e]7720;progress;set;%s;%s;%s%s\a' "$1" "$2" "$(b64 "$3")" "${4:+;$4}"; }
progress_done() { printf '\e]7720;progress;done;%s\a' "$1"; }
progress_clear() { printf '\e]7720;progress;clear%s\a' "${1:+;$1}"; }
# tab_color VALUE   (7719; empty clears)
tab_color() { printf '\e]7719;tab;color;%s\a' "$1"; }

# badge_set TEXT [TOKEN]   (7718; TOKEN is muted|accent|success|warning|error)
badge_set() { printf '\e]7718;badge;set;%s;%s\a' "$(b64 "$1")" "${2:-muted}"; }
badge_clear() { printf '\e]7718;badge;clear\a'; }
# mark_add KIND NOTE   (7722; KIND is info|warn|error)
mark_add() { printf '\e]7722;mark;add;%s;%s\a' "$1" "$(b64 "$2")"; }
mark_clear() { printf '\e]7722;mark;clear\a'; }

# key_add ID CHORD KIND PAYLOAD   (7723; KIND is send|ctl)
key_add() { printf '\e]7723;key;add;%s;%s;%s;%s\a' "$1" "$2" "$3" "$(b64 "$4")"; }
key_clear() { printf '\e]7723;key;clear%s\a' "${1:+;$1}"; }
# palette_add ID LABEL KIND PAYLOAD   (7724)
palette_add() { printf '\e]7724;palette;add;%s;%s;%s;%s\a' "$1" "$(b64 "$2")" "$3" "$(b64 "$4")"; }
palette_clear() { printf '\e]7724;palette;clear%s\a' "${1:+;$1}"; }

# fold_begin ID TITLE / fold_end ID   (7721)
fold_begin() { printf '\e]7721;fold;begin;%s;%s\a' "$1" "$(b64 "$2")"; }
fold_end() { printf '\e]7721;fold;end;%s\a' "$1"; }
# link_add ID REGEX TEMPLATE / link_clear [ID]   (7726)
link_add() { printf '\e]7726;link;add;%s;%s;%s\a' "$1" "$(b64 "$2")" "$(b64 "$3")"; }
link_clear() { printf '\e]7726;link;clear%s\a' "${1:+;$1}"; }

require_pane() {
  [ -t 1 ] || { echo "run this in a microterm pane" >&2; exit 1; }
}
