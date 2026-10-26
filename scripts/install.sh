#!/bin/sh
# microterm installer (SPEC §15.3).
#   curl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh
#   sh install.sh [--version vX.Y.Z] [--prefix DIR] [--no-modify-path] [--help]
#
# Linux: DIR/bin/microterm, DIR/share/applications, DIR/share/icons, DIR/share/doc/microterm.
# macOS: ~/Applications/microterm.app and DIR/bin/microterm -> the app's launcher
#        (never microterm-app). DIR defaults to ~/.local.
# MICROTERM_RELEASE_BASE overrides the download location (used by CI and tests; needs --version).
set -eu

REPO=0funct0ry/microterm
VERSION=${MICROTERM_VERSION:-}
PREFIX=
MODIFY_PATH=1

usage() {
  cat <<USAGE
microterm installer

Usage: install.sh [--version vX.Y.Z] [--prefix DIR] [--no-modify-path]

  --version vX.Y.Z    install this release instead of the latest
  --prefix DIR        install under DIR (default: ~/.local)
  --no-modify-path    do not add DIR/bin to your shell profile
  -h, --help          show this help
USAGE
}

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION=$2; shift 2 ;;
    --version=*) VERSION=${1#*=}; shift ;;
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; PREFIX=$2; shift 2 ;;
    --prefix=*) PREFIX=${1#*=}; shift ;;
    --no-modify-path) MODIFY_PATH=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $1" ;;
  esac
done

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

# ---- platform
os=$(uname -s); arch=$(uname -m)
case "$arch" in
  x86_64|amd64) arch=x86_64 ;;
  arm64|aarch64) arch=aarch64 ;;
  *) die "unsupported architecture: $arch" ;;
esac
case "$os" in
  Darwin) target=$arch-apple-darwin ;;
  Linux) target=$arch-unknown-linux-gnu ;;
  *) die "unsupported OS: $os (microterm supports macOS and Linux)" ;;
esac

# ---- version and URLs
if [ -n "${MICROTERM_RELEASE_BASE:-}" ]; then
  [ -n "$VERSION" ] || die "MICROTERM_RELEASE_BASE needs --version"
  base=$MICROTERM_RELEASE_BASE
else
  if [ -z "$VERSION" ]; then
    VERSION=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" | sed 's|.*/||')
    case "$VERSION" in v[0-9]*) ;; *) die "could not determine the latest release (is one published?)" ;; esac
  fi
  case "$VERSION" in v*) ;; *) VERSION=v$VERSION ;; esac
  base=https://github.com/$REPO/releases/download/$VERSION
fi
ver=${VERSION#v}
file=microterm-$ver-$target.tar.gz

tmp=$(mktemp -d "${TMPDIR:-/tmp}/microterm-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Installing microterm $VERSION for $target"
curl -fsSL "$base/$file" -o "$tmp/$file" || die "download failed: $base/$file"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "download failed: $base/SHA256SUMS"

# ---- verify
want=$(awk -v f="$file" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$want" ] || die "$file is not listed in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then got=$(sha256sum "$tmp/$file" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then got=$(shasum -a 256 "$tmp/$file" | awk '{print $1}')
else die "sha256sum or shasum is required to verify the download"; fi
[ "$want" = "$got" ] || die "checksum mismatch for $file (expected $want, got $got)"
say "Checksum OK"

mkdir "$tmp/x"
tar -xzf "$tmp/$file" -C "$tmp/x"

explicit_prefix=0
[ -n "$PREFIX" ] && explicit_prefix=1
[ -n "$PREFIX" ] || PREFIX=$HOME/.local
bindir=$PREFIX/bin
mkdir -p "$bindir"

case "$os" in
Darwin)
  if [ "$explicit_prefix" = 1 ]; then appdir=$PREFIX/Applications; else appdir=$HOME/Applications; fi
  mkdir -p "$appdir"
  rm -rf "$appdir/microterm.app"
  cp -R "$tmp/x/microterm.app" "$appdir/microterm.app"
  xattr -dr com.apple.quarantine "$appdir/microterm.app" 2>/dev/null || true
  ln -sf "$appdir/microterm.app/Contents/Resources/bin/microterm" "$bindir/microterm"
  say "Installed $appdir/microterm.app and $bindir/microterm"
  ;;
Linux)
  src=$(find "$tmp/x" -maxdepth 1 -type d -name 'microterm-*' | head -1)
  [ -n "$src" ] || die "unexpected archive layout"
  install -m 755 "$src/bin/microterm" "$bindir/microterm"
  mkdir -p "$PREFIX/share/applications" "$PREFIX/share/icons" "$PREFIX/share/doc/microterm"
  sed "s|^Exec=microterm|Exec=$bindir/microterm|" "$src/share/applications/microterm.desktop" \
    > "$PREFIX/share/applications/microterm.desktop"
  cp -R "$src/share/icons/hicolor" "$PREFIX/share/icons/"
  cp "$src/share/doc/microterm/"* "$PREFIX/share/doc/microterm/"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
  say "Installed $bindir/microterm"
  ;;
esac

# ---- PATH
case ":$PATH:" in
  *":$bindir:"*) ;;
  *)
    if [ "$MODIFY_PATH" = 1 ]; then
      case "${SHELL:-}" in
        */zsh) rc=$HOME/.zshrc; line="export PATH=\"$bindir:\$PATH\"" ;;
        */fish) rc=$HOME/.config/fish/config.fish; line="fish_add_path $bindir" ;;
        *) rc=$HOME/.profile; line="export PATH=\"$bindir:\$PATH\"" ;;
      esac
      mkdir -p "$(dirname "$rc")"
      if ! grep -qsF "$bindir" "$rc"; then
        printf '\n# microterm\n%s\n' "$line" >> "$rc"
        say "Added $bindir to PATH in $rc (open a new shell)"
      fi
    else
      say "Note: $bindir is not on your PATH"
    fi
    ;;
esac

say ""
say "Done. Run 'microterm' to start it, or 'microterm --help'."
say "Docs: https://0funct0ry.github.io/microterm/"
