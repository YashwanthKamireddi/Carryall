# Clip

**The fast, lightweight clipboard that syncs to all your devices — end-to-end encrypted.**

A tiny background daemon watches your **native OS clipboard**; everything you copy
(text, links, code, colors, images, files, video, audio) is captured, deduplicated,
and full-text searchable. Browse it in a snappy **terminal UI**, and see it on your
**phone** — all **end-to-end encrypted**, relay sees only ciphertext.

Think **clipse's lightweight TUI + ClipCascade's E2E cross-device sync + a phone client** — the combination none of them offer.

## Why it's different

| | CopyQ | clipse | ClipCascade | **Clip** |
|---|---|---|---|---|
| Lightweight TUI | ❌ heavy GUI | ✅ | ❌ | ✅ |
| Cross-device E2E sync | ❌ | ❌ | ✅ | ✅ |
| Phone client | ❌ | ❌ | partial | ✅ (PWA, no app store) |
| Footprint | large | tiny | small | **3 MB, no WebView** |

## Architecture (all Rust, zero GUI bloat)

```
 clip daemon  ──capture native clipboard──▶  SQLite + FTS5 (clip-store)
      │                                            ▲
      │ E2E (PBKDF2 + AES-256-GCM)                 │ reads
      ▼                                       clip  (ratatui TUI)
 clip-relay (axum)  ◀──ciphertext──▶  phone PWA (served by the relay)
```

- **`cli`** — the `clip` binary: `clip daemon` (background capture + sync) and `clip` (TUI).
- **`crates/clip-core`** — content model, classifier, blake3 dedup.
- **`crates/clip-store`** — SQLite + FTS5 history.
- **`crates/clip-sync`** — HLC ordering, CRDT merge, E2E crypto, chunking.
- **`server`** — relay (account-room fan-out, never decrypts) + the phone PWA.

## Usage

```bash
cargo build --release            # builds clip + clip-relay
./target/release/clip daemon &   # background capture (autostart this)
./target/release/clip            # open the TUI: search, ↑/↓, Enter to copy, Ctrl-D delete, Esc quit
```

Bind `clip` to a key in your WM (e.g. Hyprland `bind = SUPER, V, exec, foot clip`).

### Cross-device sync

```bash
./target/release/clip-relay      # or deploy it (server/DEPLOY.md) for a public HTTPS URL
CLIP_ACCOUNT=you CLIP_SECRET=pass CLIP_RELAY_URL=ws://host:8787/sync ./target/release/clip daemon
```

On your phone, open the relay URL in the browser → enter the same account + passphrase →
Add to Home Screen. Sync is **off unless `CLIP_ACCOUNT` is set** (default = local & private).

> Mobile note: phone OSes forbid background clipboard access, so on the phone you **view**
> your synced clips and **tap to copy** — automatic background capture is desktop-only.

## Tests

`cargo test` — core, store, sync (HLC/merge/crypto/chunk), relay (+ a two-device E2E test).
