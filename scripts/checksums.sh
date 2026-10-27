#!/bin/sh
# SHA256SUMS for every release artefact in DIST, after checking each one carries the licence
# files (SPEC §15.6). Fails if any artefact lacks LICENSE or THIRD-PARTY-LICENSES.html.
set -eu
DIST=${1:-dist}
cd "$DIST"
fail=0
check_archive() { # file
  list=$(tar -tzf "$1")
  for want in LICENSE THIRD-PARTY-LICENSES.html; do
    echo "$list" | grep -Eq "(^|/)$want\$" || { echo "error: $1 lacks $want" >&2; fail=1; }
  done
}
for f in microterm-*.tar.gz; do [ -e "$f" ] && check_archive "$f"; done
for f in microterm-*.dmg; do
  [ -e "$f" ] || continue
  if command -v hdiutil >/dev/null 2>&1; then
    mnt=$(mktemp -d); hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mnt" "$f"
    for want in LICENSE THIRD-PARTY-LICENSES.html; do
      [ -f "$mnt/$want" ] || { echo "error: $f lacks $want" >&2; fail=1; }
    done
    [ -f "$mnt/microterm.app/Contents/Resources/LICENSE" ] || { echo "error: $f: app lacks LICENSE" >&2; fail=1; }
    lsr=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    [ -x "$lsr" ] && "$lsr" -u "$mnt/microterm.app" >/dev/null 2>&1 || true
    hdiutil detach -quiet "$mnt"
  fi
done
for f in microterm-mux-*; do
  case "$f" in *.LICENSES.txt) continue ;; esac
  [ -e "$f" ] || continue
  [ -s "$f.LICENSES.txt" ] || { echo "error: $f lacks $f.LICENSES.txt" >&2; fail=1; }
done
[ "$fail" = 0 ] || exit 1
if command -v sha256sum >/dev/null 2>&1; then sum=sha256sum; else sum="shasum -a 256"; fi
# shellcheck disable=SC2086
ls | grep -v '^SHA256SUMS$' | grep -v '^stage-' | grep -v '^dmg-' | xargs $sum > SHA256SUMS
echo "wrote $DIST/SHA256SUMS"
