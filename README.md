<div align="center">

# Carryall

### Your clipboard, on every device — instant, private, end-to-end encrypted.

[![CI](https://github.com/YashwanthKamireddi/Carryall/actions/workflows/ci.yml/badge.svg)](https://github.com/YashwanthKamireddi/Carryall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Built with Rust](https://img.shields.io/badge/Rust-000?logo=rust&logoColor=fff)
![Linux · Windows · macOS](https://img.shields.io/badge/desktop-Linux%20·%20Windows%20·%20macOS-555)
![iOS · Android](https://img.shields.io/badge/phone-iOS%20·%20Android%20(PWA)-555)
![AES-256-GCM](https://img.shields.io/badge/E2E-AES--256--GCM-3fb950)

Copy something on one device and it’s **instantly on your other devices’ native clipboard** — ready to paste. A tiny background daemon does the work; your phone gets a clean installable web app. The server only ever sees ciphertext.

<br />

<img src="docs/onboard.png" width="38%" alt="Carryall onboarding" />
&nbsp;&nbsp;
<img src="docs/app.png" width="38%" alt="Carryall clipboard history on mobile" />

</div>

---

## Features

- **🔁 Automatic cross-device sync** — copy here, paste there. Incoming clips land straight on the native clipboard.
- **🧬 Everything you copy** — text, links, code, colors, images, **files & folders**, video, audio — full-quality, byte-for-byte.
- **🔒 End-to-end encrypted** — AES-256-GCM, key derived from your passphrase. Keys never leave your devices; the relay only moves ciphertext.
- **📱 Phone app, no App Store** — installable PWA: tap to copy, long-press to preview/edit, paste or drag files to send, search, pins, filters.
- **⚡ Two transports** — direct **LAN fast-path** (mDNS) when devices are nearby, **always-on relay** fallback for anywhere.
- **🪶 Featherweight** — a ~3.4 MB headless daemon at **~0 % idle CPU**. No GUI, no WebView, no Electron.
- **🛡️ Private by default** — sync is off until you set an account; password-manager clips are skipped.

## How it works

```
  carryall daemon ──capture native clipboard──▶  SQLite + FTS5 history
        │  ▲ apply incoming clips to the native clipboard
        │  └────────────────────────────────────────────┐
        │  end-to-end encrypted (PBKDF2 · AES-256-GCM)   │
        ├── LAN direct (mDNS-discovered TCP) ────────────┤  your other devices
        └── relay (WebSocket, ciphertext only) ──────────┤
                                                          └── phone PWA
```

The desktop daemon watches your OS clipboard and syncs each copy; the relay fans encrypted blobs between your devices and serves the phone app. Two devices on the same network skip the cloud entirely via the LAN path.

## Quick start — one command

```bash
git clone https://github.com/YashwanthKamireddi/Carryall && cd Carryall
./setup.sh        # builds, generates your key, writes config, installs autostart
```

That's it — Carryall is now capturing and syncing in the background. Add `-y` to skip every prompt. (Prefer manual? Build with `cargo build --release` and put `account`/`secret`/`relay` in `~/.config/clip/config.toml`.)

## Pair your phone in one scan

```bash
carryall pair        # prints a QR in your terminal
```

Scan it with your phone’s camera — the app opens, fills in your account, and connects. (The account + key ride in the URL **fragment**, so they never reach the server.) Then **Add to Home Screen** and you’re set; it stays signed in and reconnects on its own.

> 📵 Phone OSes block background clipboard reads, so on the phone you **view** synced clips and **tap to copy / send**. Background capture is desktop-only — a platform limit, not a shortcut.

## vs. Apple Universal Clipboard

|  | Apple Universal Clipboard | **Carryall** |
|---|---|---|
| History | last item only | **full, searchable history** |
| Lifetime | gone in ~2 min | persisted, with retention controls |
| Devices | Apple only | **Linux · Windows · macOS · iPhone · Android** |
| Reach | proximity + Bluetooth | **any network** + LAN fast-path |
| Large files | punts to AirDrop | **chunked, full-quality** files/folders/video |
| Trust | opaque | **end-to-end encrypted** — server sees ciphertext only |

## Architecture

| Crate | Responsibility |
|---|---|
| `cli` | the `carryall` binary — capture, sync (LAN + relay), apply-to-clipboard |
| `crates/clip-core` | content model, classifier, blake3 content-addressing |
| `crates/clip-store` | SQLite + FTS5 history — dedup, pins, search, retention, tombstones |
| `crates/clip-sync` | HLC ordering, CRDT merge, E2E crypto, chunking |
| `server` | the `carryall-relay` binary — encrypted fan-out (never decrypts) + the phone PWA |

## Security

- **AES-256-GCM**, key derived from your passphrase via **PBKDF2-HMAC-SHA256** — never leaves your devices.
- The relay and LAN transports carry **only ciphertext**; received blobs are integrity-checked against their content hash.
- Sync is **off until you configure an account** — fully local and private by default.

## Development

```bash
cargo test     # full workspace incl. an end-to-end sync test
cargo clippy   # CI enforces -D warnings
cargo fmt
```

## License

[MIT](LICENSE) © Yashwanth Kamireddi
