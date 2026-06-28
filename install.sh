#!/usr/bin/env sh
# Carryall — zero-manual install (no Rust needed). Downloads the prebuilt binary
# for your OS/arch, installs it, and runs first-time setup (key + autostart):
#   curl -fsSL https://raw.githubusercontent.com/YashwanthKamireddi/Carryall/main/install.sh | sh
# Windows: use install.ps1 instead.
set -eu

REPO="YashwanthKamireddi/Carryall"
BINDIR="${CARRYALL_BIN:-$HOME/.local/bin}"

err() { echo "carryall install: $*" >&2; exit 1; }

if command -v curl >/dev/null 2>&1; then DL="curl -fsSL -o";
elif command -v wget >/dev/null 2>&1; then DL="wget -qO";
else err "need curl or wget"; fi
command -v tar >/dev/null 2>&1 || err "need tar"

os=$(uname -s); arch=$(uname -m)
case "$os" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) err "unsupported OS '$os' — on Windows use install.ps1" ;;
esac
case "$arch" in
  x86_64|amd64) arch=x86_64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) err "unsupported arch '$arch'" ;;
esac

name="$os-$arch"
asset="carryall-$name.tar.gz"
url="https://github.com/$REPO/releases/latest/download/$asset"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $asset…"
$DL "$tmp/$asset" "$url" || err "download failed: $url"
tar -xzf "$tmp/$asset" -C "$tmp" || err "extract failed (corrupt download?)"

mkdir -p "$BINDIR"
cp "$tmp/carryall-$name/carryall" "$BINDIR/carryall"
cp "$tmp/carryall-$name/carryall-relay" "$BINDIR/carryall-relay" 2>/dev/null || true
chmod +x "$BINDIR/carryall" "$BINDIR/carryall-relay" 2>/dev/null || true
echo "Installed to $BINDIR/carryall"
case ":$PATH:" in
  *":$BINDIR:"*) ;;
  *) echo "Tip: add $BINDIR to your PATH (e.g. in ~/.profile)." ;;
esac

# First-run setup. Piped (curl | sh) → no TTY → setup runs zero-prompt (generates
# a key, writes config, installs autostart). Then: carryall pair.
"$BINDIR/carryall" setup "$@"
