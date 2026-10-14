#!/usr/bin/env bash
# vtebench (https://github.com/alacritty/vtebench) measures a terminal from the inside, so it
# must run in a microterm tab. SPEC §13 target: no benchmark slower than 2x Alacritty.
#   cargo install --git https://github.com/alacritty/vtebench
#   (inside microterm)  bench/vtebench.sh
set -euo pipefail
if ! command -v vtebench >/dev/null; then
  echo "vtebench not installed: cargo install --git https://github.com/alacritty/vtebench" >&2
  exit 2
fi
if [ "${TERM_PROGRAM:-}" != "microterm" ]; then
  echo "note: TERM_PROGRAM is '${TERM_PROGRAM:-unset}', not microterm; run this inside a microterm tab to measure it." >&2
fi
mkdir -p "$(dirname "$0")/results"
exec vtebench --dat "$(dirname "$0")/results/vtebench-$(date +%Y-%m-%d).dat" "$@"
