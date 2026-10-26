# Shared by the packaging scripts. Source it from the repository root.
# Inputs (all optional): VERSION, TARGET, CARGO_FEATURES, DIST.
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
HOST=$(rustc -vV | sed -n 's/^host: //p')
TARGET=${TARGET:-$HOST}
DIST=${DIST:-$ROOT/dist}
if [ -z "${VERSION:-}" ]; then
  VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' Cargo.toml | head -1)
fi
VERSION=${VERSION#v}
CARGO_FEATURES=${CARGO_FEATURES:-}
if [ "$TARGET" = "$HOST" ]; then
  BIN_DIR=$ROOT/target/release
  TARGET_ARG=
else
  BIN_DIR=$ROOT/target/$TARGET/release
  TARGET_ARG="--target $TARGET"
fi
mkdir -p "$DIST"

need_licenses() {
  for f in LICENSE THIRD-PARTY-LICENSES.html; do
    [ -f "$ROOT/$f" ] || { echo "error: $f is missing (run: make licenses)" >&2; exit 1; }
  done
}
