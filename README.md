# Clip

**The fast, lightweight clipboard that syncs to all your devices — end-to-end encrypted.**

A tiny background daemon watches your **native OS clipboard**; everything you copy
(text, links, code, colors, images, files, video, audio) is captured,
deduplicated, full-text searchable, and **synced to your other devices** — a copy
on one device lands directly on another's native clipboard. Browse and send from
your **phone** via a clean web app (no App Store). All **end-to-end encrypted** —
the relay only ever sees ciphertext.

Think **clipse's lightweight feel + ClipCascade's E2E cross-device sync + a phone
client** — the combination none of them offer.

## Why it's different

| | CopyQ | clipse | ClipCascade | **Clip** |
|---|---|---|---|---|
| Footprint | large GUI | tiny | small | **~3 MB daemon, no GUI/WebView** |
| Auto cross-device E2E sync | ❌ | ❌ | ✅ | ✅ |
| Real file/folder content sync | ❌ | ❌ | partial | ✅ (multi-file, chunked) |
| Phone client | ❌ | ❌ | partial | ✅ (PWA, no App Store) |
| LAN direct fast-path | ❌ | ❌ | ✅ | ✅ (+ relay fallback) |

## Architecture (all Rust, zero GUI)

```
 clip daemon ──capture native clipboard──▶ SQLite + FTS5 history (clip-store)
      │  ▲ apply incoming clips to clipboard
      │  └────────────────────────────────────────────┐
      │ E2E (PBKDF2 + AES-256-GCM)                     │
      ├── LAN direct (TCP, mDNS-discovered) ───────────┤  other devices
      └── relay (axum WebSocket, ciphertext only) ─────┤
                                                        └── phone PWA (served by relay)
```

- **`cli`** — the `clip` binary: a headless background **daemon** (`clip` or `clip daemon`) that captures the clipboard, syncs over LAN + relay, and writes incoming clips back to the native clipboard.
- **`crates/clip-core`** — content model, classifier, blake3 content-addressing.
- **`crates/clip-store`** — SQLite + FTS5 history (dedup, pin, search, retention).
- **`crates/clip-sync`** — HLC ordering, CRDT merge, E2E crypto, chunking.
- **`server`** — relay (account-room fan-out, never decrypts) + the phone PWA.

## Run

```bash
cargo build --release                 # builds `clip` (daemon) + `clip-relay`
./target/release/clip                 # run the background daemon (local-only by default)
```

Sync config lives in `~/.config/clip/config.toml` (or env `CLIP_ACCOUNT` /
`CLIP_SECRET` / `CLIP_RELAY_URL`):

```toml
account = "you"
secret  = "a strong passphrase"
relay   = "ws://host:8787/sync"   # optional; omit/empty for LAN-only
```

Autostart on login (Linux): `cp contrib/clip-daemon.service ~/.config/systemd/user/ && systemctl --user enable --now clip-daemon`.

### Cross-device sync

```bash
./target/release/clip-relay           # or deploy it (server/DEPLOY.md) for a public HTTPS URL
# then run the daemon with sync configured (above) on each device
```

- **LAN fast-path:** devices on the same network auto-discover via mDNS and
  transfer directly (set `CLIP_PEER=host:port` to pair manually). Relay is the
  fallback when off-network.
- **Phone:** open the relay's HTTPS URL in the browser → enter the same account +
  passphrase → Add to Home Screen. Tap a clip to copy it; tap **＋** to upload
  files/PDFs to your devices.

Sync is **off unless an account is configured** (default = local & private).

> Mobile note: phone OSes forbid background clipboard access, so on the phone you
> **view** your synced clips and **tap to copy / send** — automatic background
> capture is desktop-only.

## Security

End-to-end encrypted with AES-256-GCM; the key is derived from your passphrase
(PBKDF2-HMAC-SHA256) and never leaves your devices. The relay and LAN transports
carry only ciphertext. Received blobs are integrity-checked against their content
hash. Clipboard content marked sensitive by password managers is skipped.

## Tests

`cargo test` across the workspace — core, store, sync (HLC/merge/crypto/chunk),
the CLI (capture, blob archive + integrity, chunk reassembly, apply-gate), and
the relay (incl. a two-device end-to-end sync test).
