# Clip

**Universal cross-platform clipboard — everything you copy, on every device, faster than native.**

If it can land on the clipboard, Clip keeps it: text, links, colors, code,
rich text, images, screenshots, files, video, audio, binary. Captured
automatically, classified, deduplicated, full-text searchable, and one keystroke
from being pasted back — with **end-to-end-encrypted sync to all your devices,
anywhere** (including your iPhone, via a web app — no Mac required).

## Why it beats Apple's Universal Clipboard

| Apple Universal Clipboard | Clip |
|---|---|
| Only the most recent item | Full searchable history |
| ~2-minute lifetime | Kept until you say otherwise |
| Apple devices only | Windows · Linux · iPhone (web) · Android-ready |
| Needs proximity + Bluetooth, flaky when locked | Any network, via relay; LAN fast-path planned |
| Large files punt to AirDrop | Chunked, resumable, dedup'd blobs |
| Opaque | End-to-end encrypted — the relay only sees ciphertext |

## Architecture

```
Desktop (Tauri 2 · Rust core + SolidJS)              iPhone / web
  capture → classify → dedup → encrypted SQLite + FTS5    PWA in Safari
  blob store · global hotkey · auto-paste                 (Add to Home Screen)
        \                                                   /
         \—— E2E (PBKDF2 + AES-256-GCM) ——\     /—— E2E ——/
                                     clip-relay (Rust/axum)
                          account-room WebSocket fan-out + offline backlog
                                  (never decrypts — ciphertext only)
```

- **`crates/clip-core`** — content model, classifier, blake3 dedup (pure).
- **`crates/clip-store`** — SQLite + FTS5 history (dedup, pin, search, retention).
- **`crates/clip-sync`** — HLC ordering, CRDT merge, E2E crypto, chunking.
- **`src-tauri`** — desktop app: capture (X11 + Wayland), blob store, hotkey,
  auto-paste, tray, sync client.
- **`server`** — relay + the embedded iPhone/web PWA.

## Build & run (desktop, Linux/Windows)

```bash
pnpm install
pnpm tauri build --no-bundle      # standalone binary at target/release/clip
./target/release/clip             # summon with the bound hotkey; tray icon
```

> Use `pnpm tauri build`, **not** `cargo build` — only the former embeds the frontend.

## Sync setup

Run the relay (or deploy it — see `server/DEPLOY.md` for an always-on public URL):

```bash
./target/release/clip-relay                       # binds 0.0.0.0:8787
# desktop with sync on:
CLIP_ACCOUNT=you CLIP_SECRET=pass \
  CLIP_RELAY_URL=ws://127.0.0.1:8787/sync ./target/release/clip
# iPhone: open the relay URL in Safari, enter the same account + passphrase
```

Sync is **off unless `CLIP_ACCOUNT` is set** — default runs are fully local and private.

## Tests

`cargo test` across the workspace — core, store, sync (HLC/merge/crypto/chunk),
and the relay incl. an end-to-end two-device sync test. Browser↔Rust crypto
interop is verified with Web Crypto.

## Status

Phases 0–3 complete (capture, power UX, E2E cross-device sync) + iPhone PWA.
Native iOS/Android need a Mac / Android NDK respectively; the architecture is
mobile-ready. A full UX visual redesign is planned as a dedicated pass.
