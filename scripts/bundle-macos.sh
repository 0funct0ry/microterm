#!/bin/sh
# Builds microterm.app, a .dmg and a .tar.gz (SPEC §15.2, §15.5).
#   app binary  Contents/MacOS/microterm-app      (CFBundleExecutable)
#   launcher    Contents/Resources/bin/microterm  (what `microterm` on $PATH points at)
# Signing: APPLE_SIGNING_IDENTITY set -> Developer ID + hardened runtime; otherwise ad-hoc.
set -eu
. "$(dirname "$0")/lib-release.sh"
need_licenses
[ "$(uname -s)" = Darwin ] || { echo "error: bundle-macos.sh runs on macOS" >&2; exit 1; }
[ -f packaging/icon/generated/microterm.icns ] || scripts/gen-icons.sh

# shellcheck disable=SC2086
cargo build --release -p microterm -p mt_launcher $TARGET_ARG $CARGO_FEATURES

NAME=microterm-$VERSION-$TARGET
STAGE=$DIST/stage-$TARGET
APP=$STAGE/microterm.app
rm -rf "$STAGE"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cp "$BIN_DIR/microterm" "$APP/Contents/MacOS/microterm-app"
cp "$BIN_DIR/microterm-launcher" "$APP/Contents/Resources/bin/microterm"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist.in > "$APP/Contents/Info.plist"
cp packaging/icon/generated/microterm.icns "$APP/Contents/Resources/microterm.icns"
cp LICENSE THIRD-PARTY-LICENSES.html "$APP/Contents/Resources/"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  SIGN="--force --timestamp --options runtime --entitlements packaging/macos/entitlements.plist -s $APPLE_SIGNING_IDENTITY"
else
  SIGN="--force -s -"
fi
# shellcheck disable=SC2086
codesign $SIGN "$APP/Contents/Resources/bin/microterm"
# shellcheck disable=SC2086
codesign $SIGN "$APP/Contents/MacOS/microterm-app"
# shellcheck disable=SC2086
codesign $SIGN "$APP"
codesign --verify --strict "$APP"

cp LICENSE THIRD-PARTY-LICENSES.html "$STAGE/"
rm -f "$DIST/$NAME.tar.gz" "$DIST/$NAME.dmg"
tar -czf "$DIST/$NAME.tar.gz" -C "$STAGE" microterm.app LICENSE THIRD-PARTY-LICENSES.html

DMG=$DIST/dmg-$TARGET
rm -rf "$DMG"; mkdir -p "$DMG"
cp -R "$APP" "$DMG/"
cp LICENSE THIRD-PARTY-LICENSES.html "$DMG/"
ln -s /Applications "$DMG/Applications"
cp packaging/icon/generated/microterm.icns "$DMG/.VolumeIcon.icns"
if command -v SetFile >/dev/null 2>&1; then SetFile -a C "$DMG"; fi
hdiutil create -quiet -volname microterm -srcfolder "$DMG" -ov -format UDZO "$DIST/$NAME.dmg"
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  codesign --force --timestamp -s "$APPLE_SIGNING_IDENTITY" "$DIST/$NAME.dmg"
fi
rm -rf "$DMG"
echo "built $DIST/$NAME.dmg and $DIST/$NAME.tar.gz"
