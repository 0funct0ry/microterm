#!/bin/sh
# Builds the static microterm-mux daemon asset and its licence file (SPEC §15.2).
# musl on Linux (build on a runner of the same arch), plain release build on macOS.
set -eu
. "$(dirname "$0")/lib-release.sh"
need_licenses
case "$TARGET" in
  *-linux-*) TARGET=$(echo "$TARGET" | sed 's/-gnu$/-musl/'); rustup target add "$TARGET" >/dev/null
             BIN_DIR=$ROOT/target/$TARGET/release; TARGET_ARG="--target $TARGET" ;;
esac
# shellcheck disable=SC2086
cargo build --release -p mt_mux_server $TARGET_ARG
OUT=$DIST/microterm-mux-$VERSION-$TARGET
cp "$BIN_DIR/microterm-mux" "$OUT"
chmod +x "$OUT"
{
  cat LICENSE
  printf '\n\n----- Third-party notices -----\n\n'
  python3 -I - <<'PY'
import html, re, sys
t = open("THIRD-PARTY-LICENSES.html", encoding="utf-8").read()
t = re.sub(r"<(style|script)[^>]*>.*?</\1>", "", t, flags=re.S)
t = re.sub(r"<(li|h[1-6]|p|br|div)[^>]*>", "\n", t)
t = re.sub(r"<[^>]+>", "", t)
print(re.sub(r"\n{3,}", "\n\n", html.unescape(t)).strip())
PY
} > "$OUT.LICENSES.txt"
echo "built $OUT"
