#!/bin/sh
# Prints the release notes for a version from CHANGELOG.md: the "## vX.Y.Z" (or "## X.Y.Z")
# section, else "## Unreleased". Usage: release-notes.sh v1.0.0 [CHANGELOG.md]
set -eu
ver=${1:?usage: release-notes.sh vX.Y.Z [CHANGELOG.md]}
ver=${ver#v}
file=${2:-CHANGELOG.md}
notes=$(awk -v v="$ver" '
  /^## / { if (on) exit; h=$0; sub(/^## +/, "", h); sub(/^v/, "", h); sub(/[ \[].*$/, "", h);
           if (h == v) { on=1; next } }
  on { print }' "$file")
if [ -z "$notes" ]; then
  notes=$(awk '/^## / { if (on) exit; if ($0 ~ /^## +Unreleased/) { on=1; next } } on { print }' "$file")
fi
[ -n "$notes" ] || { echo "release-notes: no notes for $ver in $file" >&2; exit 1; }
printf '%s\n' "$notes"
