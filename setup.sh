#!/usr/bin/env sh
# Carryall — one-command setup from a fresh clone.
#   ./setup.sh        interactive: builds, configures sync, installs autostart
#   ./setup.sh -y     non-interactive: generates a key + sane defaults, no prompts
#
# After this, run `carryall pair` (or ./target/release/carryall pair) and scan
# the QR with your phone.
set -eu

cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
  echo "Carryall needs Rust. Install it from https://rustup.rs and re-run ./setup.sh" >&2
  exit 1
fi

echo "Building Carryall (release)…"
cargo build --release

exec ./target/release/carryall setup "$@"
