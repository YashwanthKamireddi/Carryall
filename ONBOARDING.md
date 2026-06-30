# Carryall — handoff for a new Claude Code chat

You're picking up **Carryall**, a working, cross-platform, end-to-end-encrypted
clipboard sync tool. This file tells you what it is, what's already deployed, how
it works, the conventions to follow, and what's left to do. Read it fully before
making changes.

---

## 1. What it is

> Your clipboard, on every device — instant, private, end-to-end encrypted.

- A **headless Rust daemon** watches the native OS clipboard (Linux/Wayland +
  X11, Windows, macOS) and syncs every copy — text, links, code, images,
  files, folders, video — to your other devices.
- A **phone client is an installable PWA** (no app store): scan a QR, "Add to
  Home Screen", done. Served by the relay itself.
- Sync goes over an **axum WebSocket relay** (plus a LAN mDNS fast-path). Every
  payload is **AES-256-GCM encrypted before it leaves the device** — the relay
  only ever sees ciphertext.
- Repo: <https://github.com/YashwanthKamireddi/Carryall> (branch `main`).
  Local path on the current machine: `/home/yk/Projects/clip`.

---

## 2. How it works (architecture)

```
copy on device A ──> daemon captures ──> gzip ──> AES-256-GCM encrypt
                                                         │
                              LAN mDNS fast-path  ◄──────┤──────►  relay (WebSocket)
                                                         │              │ rooms keyed by account
                                                         ▼              ▼ (opaque ciphertext only)
                                   device B daemon / phone PWA  ──> decrypt ──> apply to clipboard
```

- **Crypto**: PBKDF2-HMAC-SHA256 (200k iters) derives the key from your account
  name + passphrase; AES-256-GCM per frame. Salt = account bytes padded with `#`
  to ≥16 bytes — **must match** on daemon and PWA.
- **Compression**: gzip before encryption, 1-byte codec header (0=raw, 1=gzip).
  Rust `flate2` ↔ browser native `CompressionStream`. Blob chunks skip gzip.
- **Blobs** (images/files/video): 256 KiB content-addressed (blake3) chunks;
  `CLA1` archive format for multi-file selections; reassembled on the receiver.
- **Relay rooms**: one broadcast room per account + a durable SQLite **backlog**
  (recent small frames only) for offline catch-up. Large blob chunks fan out
  **live**, not parked — with a **resend protocol** so a missed chunk is
  re-requested (no stuck transfers).
- **Files on the receiver**: text/images go to the clipboard; **files/videos are
  saved to `~/Downloads`** with a notification (the Linux clipboard can't carry
  files reliably — this is the AirDrop/KDE-Connect model).

### Repo layout
- `cli/` — the daemon + CLI, binary **`carryall`** (`daemon.rs` capture+sync,
  `apply.rs` write-to-clipboard, `capture.rs`, `blobs.rs`, `lan.rs`, `proto.rs`).
- `crates/clip-core` — `Clip`/`NewClip` model, content hashing.
- `crates/clip-store` — SQLite history + FTS5 search + tombstones.
- `crates/clip-sync` — crypto + gzip (`compress.rs`).
- `server/` — the relay, binary **`carryall-relay`** (`lib.rs` router,
  `relay.rs` hub, `backlog.rs`), and the **PWA** under `server/web/`
  (`index.html`, `app.js`, `sw.js`, `manifest.webmanifest`, icons) — embedded
  into the relay binary at build time.

---

## 3. What's deployed RIGHT NOW (on the current machine)

Three `systemd --user` services:
| Service | What it does |
|---|---|
| `carryall-daemon` | captures this machine's clipboard, syncs it |
| `carryall-relay` | the WebSocket relay, listens on `127.0.0.1:8787` |
| `cloudflared` | Cloudflare Tunnel exposing the relay publicly |

- **Public relay URL**: `https://carryall.yashwanthkamireddi.dev`
  (`/health` → `ok`; WebSocket at `wss://carryall.yashwanthkamireddi.dev/sync`).
- It's a **Cloudflare Tunnel** on the user's own domain — free, no card, no
  port-forwarding. Config: `~/.cloudflared/config.yml`. Full guide:
  `RELAY-SETUP.md`.
- The daemon points at the **local** relay (`ws://127.0.0.1:8787/sync` in
  `~/.config/clip/config.toml`) since the relay runs on the same box; the phone
  reaches the **public** URL through the tunnel.
- **Account**: single user (`yk`). The account name + passphrase live in
  `~/.config/clip/config.toml` on this machine — they are personal; don't commit
  or share them. The phone must use the **same** account + passphrase to decrypt.
- Storage paths are intentionally kept as `clip` for data safety:
  `~/.config/clip/`, `~/.local/share/com.yk.clip/`.

> **If you're on a different machine now:** clone the repo, build, and either
> (a) run a daemon pointed at the existing public relay
> (`relay = "wss://carryall.yashwanthkamireddi.dev/sync"` in
> `~/.config/clip/config.toml`), or (b) stand up your own relay+tunnel per
> `RELAY-SETUP.md`. The relay only needs to run on **one** always-on machine.

---

## 4. Build / run / deploy

```bash
# Build everything
cargo build --release            # binaries: target/release/carryall, carryall-relay

# Run the daemon (foreground) / pair a phone
target/release/carryall daemon
target/release/carryall pair     # prints a QR encoding the relay URL + creds

# Run the relay locally
CLIP_RELAY_ADDR=127.0.0.1:8787 target/release/carryall-relay

# Redeploy after changes (current machine's workflow):
cargo build --release -p carryall-relay
systemctl --user stop carryall-relay
cp target/release/carryall-relay ~/.local/bin/carryall-relay
systemctl --user start carryall-relay
# (daemon is the same pattern with carryall-daemon / the `carryall` binary)
```

- The PWA is embedded in the relay binary, so **any change under `server/web/`
  requires rebuilding + redeploying `carryall-relay`**, and **bump the service
  worker cache version** in `server/web/sw.js` (currently `clip-shell-v50`) so
  phones fetch the new assets.
- `install.sh` / `setup.sh` automate first-time setup.

---

## 5. Conventions & gotchas (IMPORTANT)

- **Always `cargo fmt --all` before committing Rust** — CI fails otherwise.
- Gate before commit: `cargo fmt --all && cargo clippy --workspace
  --all-targets -- -D warnings && cargo test --workspace`.
- **Single-author commits — do NOT add a Co-Authored-By trailer.**
- **Don't build features unprompted** — propose first, then wait. The user has
  reverted unrequested additions before. Fix root causes, not band-aids.
- Keep size caps so the phone never OOMs (512 MB/blob, 2 GiB/archive).
- **Wayland**: the daemon captures via `wl-paste --watch` and writes via
  `wl-copy` / `wl-clipboard-rs` (the Wayland selection is separate from the
  XWayland one clipboard-rs uses).
- **Anti-echo**: when the daemon applies an incoming clip, it arms a content-keyed
  suppress set so its own monitor doesn't re-capture and re-broadcast it. Keyed
  on the *recomputed* dedup-id, not the incoming id (PWA ids use `web-`/`img-`/
  `f-` schemes; the daemon recomputes a blake3 hash).
- Cross-origin: the PWA stores creds + history per-origin (localStorage +
  IndexedDB), so changing the relay hostname looks like a "logout".

---

## 6. Current state — recently fixed (all on `main`, tested + verified)

The hard bugs are fixed at the root, not patched:
- **Echo loop** — phone-sent clips no longer duplicate on the desktop.
- **Clear/delete resurrection** — the daemon used to replay its *whole* history
  on every reconnect, resurrecting cleared items; now it replays **only the
  disconnect gap** (`Store::clips_since`).
- **Stuck large transfers** — a **resend protocol** (daemon + PWA) recovers
  missed blob chunks; verified with 3×5 MB PDFs.
- **Files/videos** — delivered to `~/Downloads` (real file, original name,
  collision-safe), not shoved on the clipboard as path-text.
- **PWA storage cap** — phone IndexedDB blobs bounded at 300 MB (LRU evict).
- **Sync flood / iOS icon / sign-out button / refresh flicker** — all fixed.

---

## 7. What to do next (open items)

1. **Clean slate** (optional but recommended): the user's history still holds
   old buggy duplicates + a few test clips from debugging. Clearing once from the
   phone now sticks. Or wipe the desktop's non-pinned history if asked.
2. **Open loop task** the user last raised (verify, then fix root cause):
   - pasting a **single image / multiple images from a folder** sometimes goes
     through the *files* path instead of the image path — check kind detection
     in `cli/src/capture.rs` / PWA `filesKind()`.
   - **slow image/file upload** — investigate transfer throughput (chunk
     pipelining, gzip skip on already-compressed, LAN path).
   - **search-bar paste appears with no animation/feedback** — PWA UX polish in
     `server/web/app.js` `render()` (the new-arrival pulse).
3. **Latent hardening** (low-risk, deferred): cross-device ordering uses
   wall-clock `created_at` — adopt the HLC (`crates/clip-sync/src/hlc.rs` exists)
   so a skewed clock can't mis-order or undo deletes.

---

## 8. Verify it's working

```bash
# Relay up?
curl -s https://carryall.yashwanthkamireddi.dev/health      # → ok
systemctl --user is-active carryall-daemon carryall-relay cloudflared

# End-to-end: copy text on the desktop → it appears on the phone PWA, and
# vice-versa. Send a file from the phone → it lands in ~/Downloads.
```

If the phone won't sync: reopen the PWA (picks up the latest service worker),
and re-pair with `carryall pair` if needed (scan the QR). "Can't decrypt" =
account/passphrase mismatch, not a transport problem.
