<div align="center">

# Carryall

### Your clipboard, on every device — instant, private, end-to-end encrypted.

[![CI](https://github.com/YashwanthKamireddi/Carryall/actions/workflows/ci.yml/badge.svg)](https://github.com/YashwanthKamireddi/Carryall/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Built with Rust](https://img.shields.io/badge/Rust-000?logo=rust&logoColor=fff)
![Linux · Windows · macOS](https://img.shields.io/badge/desktop-Linux%20·%20Windows%20·%20macOS-555)
![iOS · Android](https://img.shields.io/badge/phone-iOS%20·%20Android%20(PWA)-555)
![AES-256-GCM](https://img.shields.io/badge/E2E-AES--256--GCM-3fb950)

Copy something on one device and it’s **instantly on your other devices’ native clipboard** — ready to paste. A **3.4 MB headless daemon** does the work at near-zero idle CPU; your phone gets a clean installable web app. The server only ever sees ciphertext.

<br />

<img src="docs/onboard.png" width="38%" alt="Carryall onboarding" />
&nbsp;&nbsp;
<img src="docs/app.png" width="38%" alt="Carryall clipboard history on mobile" />

</div>

---

## What makes it different

Plenty of tools sync a clipboard. Carryall is the one that does **all of it at once, and weighs almost nothing**:

- **🪶 Featherweight & invisible.** A **3.4 MB** headless Rust daemon using **~6 MB RAM** and **~0% CPU when idle** (it waits on OS clipboard events — it doesn't poll). No GUI, no WebView, no Electron, no JVM. The polished competitors are Compose/JVM or native GUI apps that sit at 100 MB+.
- **📱 A phone client with no app store.** Install the PWA on **iOS *and* Android** by scanning a QR code — no APK sideloading, no App Store review, no separate builds. Most rivals are Android-only native apps, or push iOS users to a Shortcuts hack.
- **🔒 The whole combo, not just one trick.** End-to-end encryption **+** every content type (text, links, code, colors, images, files, folders, video) **+** all desktop OSes **+** a LAN fast-path **+** a self-hostable relay that reaches **any network** — in a single tool. Each competitor nails some of that; none nails all of it.

## How it works

```
  carryall daemon ──watch native clipboard──▶  SQLite + FTS5 history
        │  ▲ apply incoming clips to the native clipboard
        │  └──────────────────────────────────────────────┐
        │  gzip ─▶ AES-256-GCM  (relay sees ciphertext only)│
        ├── LAN direct (mDNS-discovered TCP) ───────────────┤  your other devices
        └── relay (WebSocket, durable backlog) ─────────────┤
                                                            └── phone PWA
```

The daemon hooks the native OS clipboard (event-driven on Wayland via `wl-paste --watch`; `clipboard-rs` on X11/Windows/macOS), classifies and content-addresses every copy with **blake3**, stores it in **SQLite + FTS5**, and syncs it. Two devices on the same network skip the cloud entirely through the **mDNS LAN fast-path**; otherwise a lightweight **axum WebSocket relay** fans encrypted blobs out and serves the phone PWA. Large files are split into 256 KiB content-addressed chunks; identical content never transfers twice.

**The speed path for a copy:** capture → **gzip** (text/code shrink 60–92%) → **AES-256-GCM** → LAN-direct *or* relay → decrypt → land on the other device’s native clipboard. Compression happens *before* encryption — ciphertext can’t be compressed, so that’s the only place a size win is real.

## Performance

Measured on this machine (Linux, release build), 2026-06:

| | Carryall |
|---|---|
| Daemon binary | **3.4 MiB** |
| Idle RAM (RSS) | **~6.4 MB** |
| Idle CPU | **~0%** — event-driven capture, no polling loop |
| gzip on a code snippet | **75% smaller** on the wire |
| gzip on a text paragraph | **92% smaller** |
| Tiny payloads (a URL) | sent **raw** — never inflated |
| Blob chunk size | 256 KiB, pipelined |
| Small-clip sync | one frame, no round-trip on an open link — perceived-instant on LAN; relay adds only network RTT |

Cross-network latency is bounded by the speed of the network (round-trip time), exactly like every relay-based tool — physics, not the app. For text and small clips you won’t perceive it.

## How it compares

Verified from each tool’s official sources (2026-06). Where a vendor doesn’t state a fact, it’s left out rather than guessed.

| Tool | Platforms | Off-network sync | E2E encrypted | Phone | Footprint | Open source |
|---|---|---|---|---|---|---|
| **Carryall** | Linux · Win · macOS · **iOS + Android (PWA)** | **Yes** (relay) **+ LAN** | **Yes** (AES-256-GCM) | **PWA, no app store** | **3.4 MB headless** | **MIT** |
| CrossPaste | Win · macOS · Linux · Android | **LAN only** | Yes | Native Android | Kotlin/Compose (JVM) | AGPL-3.0 |
| ClipCascade | Win · macOS · Linux · Android | Yes (relay) | Yes | Native Android | Java server (JVM) | GPL-3.0 |
| SyncClipboard | Win · macOS · Linux · iOS¹ · Android | Yes (self-host) | **No** (manual HTTPS) | Android app; iOS Shortcuts | C# / ASP.NET | MIT |
| Ditto | **Windows only** | LAN only | — | None | Native | GPL-3.0 |
| CopyQ | Win · macOS · Linux | **No device sync** | — | None | C++/Qt | GPL-3.0 |
| Maccy | **macOS only** | iCloud (Mac↔Mac) | Yes (iCloud) | None | Native | MIT |
| Paste | macOS · iOS/iPadOS | iCloud | — | Native iOS | Proprietary | No |
| Windows Clipboard | Windows (+Android via SwiftKey) | **1 clip, ~1 hour** | — | Android only | OS feature | No |
| Apple Universal Clipboard | Apple devices only | Handoff (proximity) | Yes | Apple only | OS feature | No |

<sup>¹ SyncClipboard on iOS is via the Shortcuts app, not a background client.</sup>

**Where Carryall genuinely leads**
- **Off-network *and* E2E *and* cross-platform mobile, together.** CrossPaste & Ditto are LAN-only; SyncClipboard has no built-in E2E; Apple/Windows are ecosystem-locked with no real history. Carryall is the only one combining any-network relay + LAN fast-path + E2E + a phone client that installs on both iOS and Android.
- **Footprint.** A 3.4 MB headless daemon at ~6 MB RAM is in a different weight class from the JVM and native-GUI options.
- **Phone story.** One PWA, both mobile OSes, installed by scanning a QR — no app store, no sideload.

**Where it ties** — E2E (CrossPaste, ClipCascade, Maccy, Apple also have it), cross-platform *desktop* (CrossPaste, ClipCascade, SyncClipboard, CopyQ), and full searchable history (most managers).

**Where it honestly loses** — **maturity:** CopyQ, Maccy, and SyncClipboard have years of polish and thousands of users; Carryall is new. **Desktop recall:** CopyQ/Maccy/Paste give you a rich on-desktop palette to scroll old clips — Carryall is headless, so you browse history on your phone. **Native mobile:** CrossPaste and ClipCascade ship native Android apps; Carryall’s phone client is a PWA (which is also why it reaches iOS so easily).

## Honest limitations

- **No desktop pop-up palette.** The desktop side is a pure background daemon. History recall lives on the phone PWA. If you want a Spotlight-style local clip palette, a dedicated desktop manager will feel better there.
- **Phone capture is foreground-only.** iOS and Android both block background clipboard reads — so on the phone you **view** synced clips and **tap to copy / send**, rather than it silently capturing. That’s an OS limit every tool shares, not a shortcut.
- **Cross-network speed is RTT-bound.** On the same Wi-Fi it’s effectively instant (LAN path); across the internet it’s as fast as the round trip allows.
- **Linux is battle-tested; Windows/macOS are not yet.** The code compiles cleanly and uses the same cross-platform backends on all three, but day-to-day testing so far has been on Linux.

## Quick start — one command

```bash
git clone https://github.com/YashwanthKamireddi/Carryall && cd Carryall
./setup.sh        # builds, generates your key, writes config, installs autostart
```

That’s it — Carryall is now capturing and syncing in the background. Add `-y` to skip every prompt. (Prefer manual? `cargo build --release`, then put `account`/`secret`/`relay` in `~/.config/clip/config.toml`.)

### Pair your phone in one scan

```bash
carryall pair        # prints a QR in your terminal
```

Scan it with your phone’s camera — the app opens, fills in your account, and connects. The account + key ride in the URL **fragment**, so they never reach the server. Then **Add to Home Screen** and you’re set; it stays signed in and reconnects on its own.

## Security

- **AES-256-GCM**, key derived from your passphrase via **PBKDF2-HMAC-SHA256** — the key never leaves your devices.
- The relay and LAN transports carry **only ciphertext**; payloads are gzip-compressed *before* encryption, and every received blob is integrity-checked against its content hash.
- Sync is **off until you configure an account** — fully local and private by default.

## Is this a product?

Today, Carryall is a **world-class personal / self-hosted tool**: you run the relay (a single small binary, or Docker), it’s yours end to end, and it costs nothing. Turning it into a *hosted public product* — managed relays, accounts, billing, support, abuse handling — is a separate and much bigger decision. The architecture is built to allow it (the relay scales horizontally and only ever stores ciphertext), but that’s a deliberate future step, not a claim being made today.

## Architecture

| Crate | Responsibility |
|---|---|
| `cli` | the `carryall` binary — capture, sync (LAN + relay), apply-to-clipboard |
| `crates/clip-core` | content model, classifier, blake3 content-addressing |
| `crates/clip-store` | SQLite + FTS5 history — dedup, pins, search, retention, tombstones |
| `crates/clip-sync` | HLC ordering, CRDT merge, E2E crypto, gzip, chunking |
| `server` | the `carryall-relay` binary — encrypted fan-out (never decrypts) + the phone PWA |

## Development

```bash
cargo test     # full workspace incl. an end-to-end sync test
cargo clippy   # CI enforces -D warnings
cargo fmt
```

## License

[MIT](LICENSE) © Yashwanth Kamireddi
