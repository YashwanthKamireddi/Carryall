<div align="center">

# Carryall

### Your clipboard, on every device — instant, private, end-to-end encrypted.

[![CI](https://github.com/YashwanthKamireddi/Carryall/actions/workflows/ci.yml/badge.svg)](https://github.com/YashwanthKamireddi/Carryall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust&logoColor=white)
![Platforms](https://img.shields.io/badge/desktop-Linux%20·%20Windows%20·%20macOS-informational)
![Phone](https://img.shields.io/badge/phone-iOS%20·%20Android%20PWA-informational)
![E2E](https://img.shields.io/badge/E2E-AES--256--GCM-success)
![Footprint](https://img.shields.io/badge/daemon-~3.4%20MB%20·%20no%20GUI-brightgreen)

**Copy on one device → it lands on your other devices' native clipboard.**
A tiny background daemon, no GUI, no Electron. The server only ever sees ciphertext.

</div>

<!-- Drop a short screen-capture GIF here once recorded: ![demo](docs/demo.gif) -->

---

## What it is

Carryall is a lightweight **background daemon** that watches your **native OS clipboard**.
Everything you copy — text, links, code, colors, **images, files, folders, video, audio** —
is captured, deduplicated, full-text searchable, and **synced to your other devices**,
landing directly on their native clipboard. Browse and send from your **phone** through a
clean installable web app — no App Store. Every byte is **end-to-end encrypted**; the relay
and the network only ever carry ciphertext.

> Think *clipse's* lightweight feel **+** *ClipCascade's* cross-device E2E sync **+** a real
> phone client — the combination none of them offer.

## Built to beat Apple's Universal Clipboard

| | Apple Universal Clipboard | **Carryall** |
|---|---|---|
| History | only the **last** item | **full searchable history**, synced |
| Lifetime | item gone in ~2 min | persisted (with retention controls) |
| Devices | **Apple only** | **Linux · Windows · macOS · iPhone · Android** |
| Reach | proximity + Bluetooth, flaky when locked | **any network** (relay) + **LAN fast-path** |
| Large files | punts to AirDrop | **chunked, full-quality** file/folder/video sync |
| Trust | opaque | **end-to-end encrypted** — server sees only ciphertext |

## Features

- 🔁 **Automatic cross-device sync** — copy here, paste there; incoming clips land on the native clipboard.
- 🧬 **Every content type** — text, links, code, colors, images, **files & folders (recursive)**, video, audio.
- 🔒 **End-to-end encrypted** — AES‑256‑GCM, key derived from your passphrase (PBKDF2). Keys never leave your devices.
- ⚡ **Two transports** — direct **LAN fast-path** (mDNS-discovered) with an **always-on relay** fallback for anywhere.
- 📱 **Phone client, no App Store** — an installable PWA: tap to copy, paste/drag files to send, full-text search, pin, type filters.
- 🪶 **Tiny & quiet** — a ~3.4 MB headless daemon, **~0% idle CPU**, no GUI, no WebView, no Electron.
- 🔎 **Searchable history** — SQLite + FTS5, pins, type filters, retention pruning.
- 🛡️ **Private by default** — sync is off until you configure an account; password‑manager‑sensitive clips are skipped.

## Quick start

```bash
# Build the daemon + relay (one optimized Rust workspace)
cargo build --release

# Run the background daemon (local-only until you configure sync)
./target/release/carryall
```

Configure sync in `~/.config/clip/config.toml` (or via `CLIP_ACCOUNT` / `CLIP_SECRET` / `CLIP_RELAY_URL`):

```toml
account = "you"
secret  = "a strong passphrase"   # this IS your encryption key — pick a strong one
relay   = "ws://host:8787/sync"   # optional; omit for LAN-only
```

**Autostart on login (Linux):**

```bash
cp contrib/carryall-daemon.service ~/.config/systemd/user/
systemctl --user enable --now carryall-daemon
```

## Cross-device sync

```bash
./target/release/carryall-relay        # run the relay (see server/DEPLOY.md for a public HTTPS URL)
# then run the daemon with sync configured (above) on each device
```

- **LAN fast-path** — devices on the same network auto-discover over mDNS and transfer
  directly (set `CLIP_PEER=host:port` to pair manually). Lowest latency, no cloud hop.
- **Relay** — when you're off-network, a lightweight WebSocket relay fans ciphertext between
  your devices. It can never read your clips.
- **Phone** — open the relay's HTTPS URL → enter the same account + passphrase → **Add to
  Home Screen**. Tap a clip to copy; tap **＋** (or paste/drag) to send files to your devices.

> 📵 **Mobile reality:** phone OSes forbid background clipboard reads, so on the phone you
> **view** synced clips and **tap to copy / send** — automatic background capture is desktop-only.
> This is a hard OS limit, not a shortcut.

## How it works

```
  carryall daemon ──capture native clipboard──▶  SQLite + FTS5 history (clip-store)
      │  ▲ apply incoming clips to the native clipboard
      │  └──────────────────────────────────────────────┐
      │  E2E (PBKDF2 + AES-256-GCM)                       │
      ├── LAN direct (TCP, mDNS-discovered) ──────────────┤  your other devices
      └── relay (axum WebSocket, ciphertext only) ────────┤
                                                          └── phone PWA (served by the relay)
```

| Crate | Responsibility |
|---|---|
| **`cli`** | the `carryall` binary — headless daemon: capture, sync (LAN + relay), apply-to-clipboard |
| **`crates/clip-core`** | content model, classifier, blake3 content-addressing |
| **`crates/clip-store`** | SQLite + FTS5 history — dedup, pin, search, retention, tombstones |
| **`crates/clip-sync`** | HLC ordering, CRDT merge, E2E crypto, chunking |
| **`server`** | the `carryall-relay` binary — account-room fan-out (never decrypts) + the phone PWA |

## Security

- **End-to-end encrypted** with **AES‑256‑GCM**; the key is derived from your passphrase via
  **PBKDF2‑HMAC‑SHA256** and never leaves your devices.
- The **relay and LAN transports carry only ciphertext** — the server can't read anything.
- Received blobs are **integrity-checked** against their content hash before use.
- Clipboard content flagged sensitive by **password managers is skipped**.
- Sync is **off until an account is configured** — default is fully local and private.

## Performance & footprint

- **~3.4 MB** statically-optimized daemon (LTO, `panic=abort`, stripped) — no GUI, no WebView.
- **~0% idle CPU**, even with a large image parked on the clipboard (change-gated polling).
- **Full-quality** transfers — images, files, folders, and video sync **byte-for-byte**, chunked,
  with bounded memory on the receive path.

## Development

```bash
cargo test          # full workspace: core, store, sync, daemon, relay (+ end-to-end sync test)
cargo clippy        # lint-clean (CI enforces -D warnings)
cargo fmt           # rustfmt-clean
```

## License

[MIT](LICENSE) © Yashwanth Kamireddi
