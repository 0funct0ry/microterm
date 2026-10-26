#!/bin/sh
# Builds the Linux tarball: bin/microterm, .desktop file, hicolor icons, licences (SPEC §15.2).
set -eu
. "$(dirname "$0")/lib-release.sh"
need_licenses
[ -d packaging/icon/generated/hicolor ] || scripts/gen-icons.sh

# shellcheck disable=SC2086
cargo build --release -p microterm $TARGET_ARG $CARGO_FEATURES

NAME=microterm-$VERSION-$TARGET
STAGE=$DIST/stage-$TARGET/$NAME
rm -rf "$DIST/stage-$TARGET"
mkdir -p "$STAGE/bin" "$STAGE/share/applications" "$STAGE/share/icons" "$STAGE/share/doc/microterm"
cp "$BIN_DIR/microterm" "$STAGE/bin/microterm"
cp packaging/linux/microterm.desktop "$STAGE/share/applications/microterm.desktop"
cp -R packaging/icon/generated/hicolor "$STAGE/share/icons/hicolor"
cp LICENSE THIRD-PARTY-LICENSES.html "$STAGE/share/doc/microterm/"
# Licences also at the archive root, next to bin/ (SPEC §15.6).
cp LICENSE THIRD-PARTY-LICENSES.html "$STAGE/"
rm -f "$DIST/$NAME.tar.gz"
tar -czf "$DIST/$NAME.tar.gz" -C "$DIST/stage-$TARGET" "$NAME"
echo "built $DIST/$NAME.tar.gz"
