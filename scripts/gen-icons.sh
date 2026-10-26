#!/bin/sh
# Generate every icon artefact from the approved master (packaging/icon/microterm*.svg).
# Needs rsvg-convert and ImageMagick; iconutil (macOS only) builds the .icns.
set -eu
cd "$(dirname "$0")/.."
SRC=packaging/icon
OUT=packaging/icon/generated
SITE=site/public
rm -rf "$OUT"; mkdir -p "$OUT/hicolor" "$OUT/microterm.iconset" "$SITE"

render() { # size outfile  (small variant below 64px)
  if [ "$1" -lt 64 ]; then src="$SRC/microterm-small.svg"; else src="$SRC/microterm.svg"; fi
  rsvg-convert -w "$1" -h "$1" "$src" -o "$2"
}

for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$OUT/hicolor/${s}x${s}/apps"; render $s "$OUT/hicolor/${s}x${s}/apps/microterm.png"
done
mkdir -p "$OUT/hicolor/scalable/apps"; cp "$SRC/microterm.svg" "$OUT/hicolor/scalable/apps/microterm.svg"
render 1024 "$OUT/microterm-1024.png"

# macOS iconset -> .icns
for s in 16 32 128 256 512; do
  render $s "$OUT/microterm.iconset/icon_${s}x${s}.png"
  render $((s*2)) "$OUT/microterm.iconset/icon_${s}x${s}@2x.png"
done
if command -v iconutil >/dev/null 2>&1; then
  iconutil -c icns "$OUT/microterm.iconset" -o "$OUT/microterm.icns"
fi

# Website
cp "$SRC/microterm.svg" "$SITE/logo.svg"
cp "$SRC/microterm-small.svg" "$SITE/favicon.svg"
render 180 "$SITE/apple-touch-icon.png"
render 512 "$SITE/icon-512.png"
magick "$OUT/hicolor/16x16/apps/microterm.png" "$OUT/hicolor/32x32/apps/microterm.png" \
  "$OUT/hicolor/48x48/apps/microterm.png" "$SITE/favicon.ico"
# Social card 1200x630
magick -size 1200x630 xc:'#21252b' \( "$OUT/microterm-1024.png" -resize 360x360 \) -gravity west -geometry +120+0 -composite \
  -font Helvetica-Bold -pointsize 96 -fill '#abb2bf' -gravity west -annotate +540-30 'microterm' \
  -font Helvetica -pointsize 36 -fill '#61afef' -annotate +540+60 'A minimal GPU-accelerated terminal' "$SITE/og.png" 2>/dev/null || true

# In-app copies (embedded with include_bytes!)
mkdir -p assets/app-icon
cp "$OUT/microterm-1024.png" assets/app-icon/microterm-1024.png
render 256 assets/app-icon/microterm-256.png
echo "icons generated in $OUT and $SITE"
