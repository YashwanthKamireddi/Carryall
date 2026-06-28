//! Background daemon: watches the native OS clipboard, stores every copy, and
//! (if configured) E2E-syncs it to the relay so it appears on your other
//! devices. Lightweight — no GUI, no WebView.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use clip_core::{Clip, NewClip};
use clip_store::Store;
use clip_sync::{decrypt, derive_key, encrypt, pack, unpack, Key};
use clipboard_rs::{ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use crate::capture;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Shared state for the daemon's capture + sync paths.
#[derive(Clone)]
pub struct Ctx {
    pub store: Arc<Mutex<Store>>,
    pub blob_dir: PathBuf,
    pub outgoing: broadcast::Sender<Clip>,
    /// Content-hash the monitor should ignore once — set right before we write
    /// an incoming synced clip to the native clipboard, so we don't re-capture
    /// and re-broadcast our own write (which would loop between devices).
    pub suppress: Arc<Mutex<Option<String>>>,
    /// Daemon start time (ms). Only clips created at/after this auto-apply to
    /// the native clipboard — so a relay backlog replay on reconnect refills
    /// history WITHOUT clobbering whatever is currently on your clipboard.
    pub started: i64,
    /// Recently-applied clip ids (bounded) — guards against double-apply when
    /// the same clip arrives via multiple paths (relay + LAN) or a backlog
    /// replay, without unbounded memory growth on a long-running daemon.
    pub applied: Arc<Mutex<crate::proto::Dedup>>,
}

impl Ctx {
    /// True if `id` was just applied by us and should be skipped by the monitor.
    fn take_suppressed(&self, id: &str) -> bool {
        let mut g = self.suppress.lock().unwrap();
        if g.as_deref() == Some(id) {
            *g = None;
            return true;
        }
        false
    }
}

/// Data directory (kept compatible with prior history at com.yk.clip).
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLIP_DATA_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.yk.clip")
}

impl Ctx {
    /// Persist a freshly captured clip (dedup + blob externalize) and forward to sync.
    fn ingest(&self, new_clip: NewClip) {
        let new_clip = crate::blobs::externalize(&self.blob_dir, new_clip);
        // A file/folder selection too large to content-sync (e.g. a copied game
        // or a multi-GB folder, past the blob/archive caps) ends up with no blob.
        // Keep it in LOCAL history, but don't sync un-downloadable names to other
        // devices — so copying huge things locally stays local.
        let syncable = is_syncable(new_clip.kind, new_clip.blob_ref.is_some());
        let now = now_ms();
        let stored = {
            let store = match self.store.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            match store.insert(new_clip, now) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("carryall: store insert failed: {e}");
                    return;
                }
            }
        };
        if syncable {
            let _ = self.outgoing.send(stored);
        }
    }
}

/// Run the daemon forever.
pub async fn run() {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);

    // Single-instance: don't run two daemons against the same store.
    let pidfile = dir.join("daemon.pid");
    if let Ok(s) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                eprintln!("carryall: already running (pid {pid}) — exiting");
                return;
            }
        }
    }
    let _ = std::fs::write(&pidfile, std::process::id().to_string());

    let store = match Store::open(dir.join("clip.db")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("carryall: cannot open store: {e}");
            return;
        }
    };
    let blob_dir = dir.join("blobs");
    let _ = std::fs::create_dir_all(&blob_dir);
    // Generous buffer so a burst of copies while the sync link is momentarily
    // busy doesn't lag the channel; anything that still slips past is recovered
    // by the recent-clip replay on reconnect (see `sync_connection`).
    let (outgoing, _) = broadcast::channel(1024);
    let ctx = Ctx {
        store: Arc::new(Mutex::new(store)),
        blob_dir,
        outgoing,
        suppress: Arc::new(Mutex::new(None)),
        started: now_ms(),
        applied: Arc::new(Mutex::new(crate::proto::Dedup::new(8192))),
    };

    spawn_monitor(ctx.clone());
    eprintln!("carryall: capturing clipboard");

    if let Some(cfg) = SyncConfig::load() {
        eprintln!("carryall: sync enabled (account={})", cfg.account);
        // LAN peer-to-peer fast-path (direct, low-latency). Additive to relay.
        let port = std::env::var("CLIP_LAN_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::lan::DEFAULT_PORT);
        let peer = std::env::var("CLIP_PEER").ok().filter(|s| !s.is_empty());
        // Account fingerprint (not the name) scopes mDNS discovery to your devices.
        let acct_fp =
            clip_core::content_hash(clip_core::ContentKind::Binary, cfg.account.as_bytes())[..16]
                .to_string();
        crate::lan::spawn(
            ctx.clone(),
            cfg.key.clone(),
            port,
            peer,
            cfg.device.clone(),
            acct_fp,
        );
        // Relay path (cross-network). Set CLIP_RELAY_URL="" to run LAN-only.
        let relay_disabled = matches!(std::env::var("CLIP_RELAY_URL"), Ok(s) if s.is_empty());
        if relay_disabled {
            eprintln!("carryall: relay disabled (LAN-only)");
        } else {
            spawn_sync(ctx.clone(), cfg);
        }
    } else {
        eprintln!("carryall: local-only — run `carryall setup` to sync to your devices");
    }

    // Periodic retention prune (non-pinned > 30 days).
    let prune_ctx = ctx.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(store) = prune_ctx.store.lock() {
                let _ = store.prune_older_than(now_ms() - 30 * 24 * 60 * 60 * 1000);
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
        }
    });

    let _ = tokio::signal::ctrl_c().await;
}

// ---- clipboard monitor ----

fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

fn spawn_monitor(ctx: Ctx) {
    #[cfg(target_os = "linux")]
    if is_wayland() {
        std::thread::spawn(move || wayland_loop(ctx));
        return;
    }
    std::thread::spawn(move || clipboard_rs_loop(ctx));
}

struct Handler {
    ctx: ClipboardContext,
    daemon: Ctx,
    last_id: Option<String>,
}
impl ClipboardHandler for Handler {
    fn on_clipboard_change(&mut self) {
        let Some(nc) = capture::from_clipboard_rs(&self.ctx) else {
            return;
        };
        let id = nc.dedup_id();
        if self.last_id.as_deref() == Some(id.as_str()) {
            return;
        }
        // Don't re-capture a clip we just wrote from an incoming sync.
        if self.daemon.take_suppressed(&id) {
            self.last_id = Some(id);
            return;
        }
        self.last_id = Some(id);
        self.daemon.ingest(nc);
    }
}

fn clipboard_rs_loop(daemon: Ctx) {
    let ctx = match ClipboardContext::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("carryall: clipboard init failed: {e}");
            return;
        }
    };
    let handler = Handler {
        ctx,
        daemon,
        last_id: None,
    };
    let mut watcher = match ClipboardWatcherContext::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("carryall: watcher init failed: {e}");
            return;
        }
    };
    watcher.add_handler(handler);
    watcher.start_watch();
}

#[cfg(target_os = "linux")]
fn wayland_loop(daemon: Ctx) {
    // Prefer event-driven capture (`wl-paste --watch`): zero idle wakeups and
    // instant capture, vs. polling. wl-clipboard is already required on Wayland
    // (we apply clips with wl-copy), so this adds no new dependency. If it isn't
    // available we poll; if the watcher dies (e.g. compositor restart) we retry,
    // and only poll permanently when watching can't start at all.
    loop {
        if !run_wayland_watch(&daemon) {
            wayland_poll_loop(&daemon); // never returns — wl-paste unavailable
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Apply the current Wayland clipboard once: read it, and ingest unless it's a
/// duplicate or a clip we just wrote ourselves. Shared by the event + poll paths.
#[cfg(target_os = "linux")]
fn capture_once(daemon: &Ctx, last_id: &mut Option<String>) {
    if let Some(nc) = read_wayland() {
        let id = nc.dedup_id();
        if last_id.as_deref() != Some(id.as_str()) {
            *last_id = Some(id.clone());
            if !daemon.take_suppressed(&id) {
                daemon.ingest(nc);
            }
        }
    }
}

/// Event-driven capture via `wl-paste --watch`, which blocks on the wlr
/// data-control protocol and emits one line per selection change (and once at
/// start). Returns `true` if the watcher ran (and later exited), `false` if it
/// couldn't be spawned (so the caller falls back to polling).
#[cfg(target_os = "linux")]
fn run_wayland_watch(daemon: &Ctx) -> bool {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let mut child = match Command::new("wl-paste")
        .args(["--watch", "echo", ""]) // one (blank) line per clipboard change
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false, // wl-paste not installed → caller polls
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return false;
    };
    let mut last_id: Option<String> = None;
    for line in BufReader::new(stdout).lines() {
        if line.is_err() {
            break; // pipe closed → watcher died
        }
        capture_once(daemon, &mut last_id);
    }
    let _ = child.kill();
    let _ = child.wait();
    true
}

/// Fallback capture: poll the clipboard, gated by a cheap capped probe so a large
/// payload parked on the clipboard costs almost nothing to re-check. Never
/// returns. Used only when `wl-paste --watch` is unavailable.
#[cfg(target_os = "linux")]
fn wayland_poll_loop(daemon: &Ctx) -> ! {
    let mut last_sig: Option<Vec<u8>> = None;
    let mut last_id: Option<String> = None;
    loop {
        let sig = clipboard_signal();
        if sig != last_sig {
            last_sig = sig.clone();
            if sig.is_some() {
                capture_once(daemon, &mut last_id);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// Largest prefix we read to detect a clipboard change. Content at or below this
/// is captured exactly; only larger payloads rely on a prefix probe. Kept small
/// so a large payload parked on the clipboard costs almost nothing to re-probe
/// each idle poll, while still exactly covering essentially all text clips.
#[cfg(target_os = "linux")]
const PROBE_CAP: u64 = 64 * 1024;

/// A cheap change-detection signal for the Wayland clipboard: the sorted mime
/// types plus at most `PROBE_CAP` bytes of the primary content. `None` means
/// nothing to capture (empty, or password-manager-sensitive). Reading only a
/// capped prefix keeps idle polling from transferring a large payload every
/// cycle; pipe backpressure means we only pull what we read.
#[cfg(target_os = "linux")]
fn clipboard_signal() -> Option<Vec<u8>> {
    use std::io::Read;
    use wl_clipboard_rs::paste::{get_contents, get_mime_types, ClipboardType, MimeType, Seat};
    let types = get_mime_types(ClipboardType::Regular, Seat::Unspecified).ok()?;
    if types.is_empty() {
        return None;
    }
    const SENSITIVE: &[&str] = &["passwordmanagerhint", "x-kde-passwordmanager", "concealed"];
    if types.iter().any(|t| {
        let t = t.to_ascii_lowercase();
        SENSITIVE.iter().any(|h| t.contains(h))
    }) {
        return None;
    }
    let has = |m: &str| types.iter().any(|t| t == m);
    // Same primary-content selection as read_wayland, so the signal tracks the
    // exact bytes that would be captured.
    let mime = if has("image/png") {
        MimeType::Specific("image/png")
    } else if has("text/uri-list") {
        MimeType::Specific("text/uri-list")
    } else {
        MimeType::Text
    };
    // Lead the signal with the sorted mime types so a type change is detected
    // even if the probe bytes happen to match.
    let mut sig: Vec<u8> = Vec::new();
    let mut sorted: Vec<&String> = types.iter().collect();
    sorted.sort();
    for t in sorted {
        sig.extend_from_slice(t.as_bytes());
        sig.push(0);
    }
    sig.push(0xFF);
    let (pipe, _) = get_contents(ClipboardType::Regular, Seat::Unspecified, mime).ok()?;
    pipe.take(PROBE_CAP).read_to_end(&mut sig).ok()?;
    Some(sig)
}

#[cfg(target_os = "linux")]
fn read_wayland() -> Option<NewClip> {
    use std::io::Read;
    use wl_clipboard_rs::paste::{get_contents, get_mime_types, ClipboardType, MimeType, Seat};
    let types = get_mime_types(ClipboardType::Regular, Seat::Unspecified).ok()?;
    const SENSITIVE: &[&str] = &["passwordmanagerhint", "x-kde-passwordmanager", "concealed"];
    if types.iter().any(|t| {
        let t = t.to_ascii_lowercase();
        SENSITIVE.iter().any(|h| t.contains(h))
    }) {
        return None;
    }
    let has = |m: &str| types.iter().any(|t| t == m);
    let read = |mime: MimeType| -> Option<Vec<u8>> {
        let (mut pipe, _) = get_contents(ClipboardType::Regular, Seat::Unspecified, mime).ok()?;
        let mut buf = Vec::new();
        pipe.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    if has("image/png") {
        return capture::image_clip(read(MimeType::Specific("image/png"))?);
    }
    if has("text/uri-list") {
        let bytes = read(MimeType::Specific("text/uri-list"))?;
        let text = String::from_utf8_lossy(&bytes);
        let files = text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();
        return capture::files_clip(files);
    }
    let plain = String::from_utf8_lossy(&read(MimeType::Text)?).into_owned();
    // Carry HTML when present, so a rich-text copy keeps formatting on sync.
    let html = if has("text/html") {
        read(MimeType::Specific("text/html"))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .filter(|h| !h.trim().is_empty())
    } else {
        None
    };
    capture::text_clip_html(&plain, html)
}

// ---- sync client ----

pub struct SyncConfig {
    pub account: String,
    pub device: String,
    pub url: String,
    pub key: Key,
}
impl SyncConfig {
    /// Load from environment first, then `~/.config/clip/config.toml`
    /// (simple `key = value` lines). Sync is off if no `account` is found.
    pub fn load() -> Option<SyncConfig> {
        let cfg = read_config();
        let get = |env: &str, key: &str| {
            std::env::var(env)
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| cfg.get(key).cloned())
        };
        let account = get("CLIP_ACCOUNT", "account")?;
        let secret = get("CLIP_SECRET", "secret").unwrap_or_else(|| account.clone());
        let device = get("CLIP_DEVICE", "device")
            .unwrap_or_else(|| format!("device-{}", std::process::id()));
        let url =
            get("CLIP_RELAY_URL", "relay").unwrap_or_else(|| "ws://127.0.0.1:8787/sync".into());
        let key = derive_key(secret.as_bytes(), &account_salt(&account)).ok()?;
        Some(SyncConfig {
            account,
            device,
            url,
            key,
        })
    }
}

/// Derive the PBKDF2 salt from the account: the account's UTF-8 BYTES padded with
/// `#` to at least 16 bytes. The PWA must pad bytes (not JS chars) identically, or
/// a non-ASCII account (e.g. "café") would derive a different key and fail to
/// decrypt across desktop↔phone.
fn account_salt(account: &str) -> Vec<u8> {
    let mut salt = account.as_bytes().to_vec();
    while salt.len() < 16 {
        salt.push(b'#');
    }
    salt
}

/// `carryall pair` — print a QR (and URL) that pairs a phone in one scan. The QR
/// encodes the PWA origin with account/secret/relay in the URL **fragment**
/// (`#k=…`), which browsers never send to the server — so the secret stays
/// client-side. The phone scans it, the PWA auto-fills and connects.
pub fn print_pairing() {
    use base64::{engine::general_purpose::URL_SAFE, Engine};
    let cfg = read_config();
    let get = |env: &str, key: &str| {
        std::env::var(env)
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| cfg.get(key).cloned())
    };
    let Some(account) = get("CLIP_ACCOUNT", "account") else {
        eprintln!(
            "carryall: no account configured — set account/secret in {} first.",
            config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
        return;
    };
    let secret = get("CLIP_SECRET", "secret").unwrap_or_else(|| account.clone());
    let relay = get("CLIP_RELAY_URL", "relay").unwrap_or_else(|| "ws://127.0.0.1:8787/sync".into());

    // PWA origin = the relay's http(s) host, minus the /sync path.
    let origin = relay
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    let origin = origin
        .strip_suffix("/sync")
        .unwrap_or(&origin)
        .trim_end_matches('/');
    let payload = serde_json::json!({ "a": account, "s": secret, "r": relay }).to_string();
    let url = format!("{origin}/#k={}", URL_SAFE.encode(payload));

    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => {
            let qr = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            println!("\n  Scan with your phone's camera to pair Carryall:\n\n{qr}\n  {url}\n");
        }
        Err(_) => println!("\n  Open this on your phone to pair Carryall:\n\n  {url}\n"),
    }
}

/// Path to the user config file.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("clip").join("config.toml"))
}

/// Minimal `key = value` config reader (no TOML dep — stays lean).
fn read_config() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(content) = config_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
            }
        }
    }
    map
}

/// Percent-encode a query value (RFC 3986 unreserved set kept). The PWA encodes
/// account/device with `encodeURIComponent`; the relay percent-decodes the query,
/// so the daemon MUST encode too or an account with a space/`&`/`+`/`#` would land
/// in a different room (or fail to connect) — silently breaking desktop↔phone sync.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// How many recent clips to re-offer on each (re)connect. The receiver dedups,
/// so this is pure catch-up insurance for clips captured while the link was down.
const REPLAY_ON_CONNECT: i64 = 512;

/// Blob chunks queued before forcing a socket flush — overlaps our encrypt work
/// with the socket drain and cuts syscalls, while bounding buffered memory.
const SEND_WINDOW: usize = 8;

/// Whether a clip should be offered to other devices. A file/video/audio clip
/// whose content was too large to externalize into a blob stays local-only
/// (we'd otherwise sync un-downloadable file names). Mirrors the rule in
/// [`Ctx::ingest`] so capture and replay agree.
fn is_syncable(kind: clip_core::ContentKind, has_blob: bool) -> bool {
    use clip_core::ContentKind::{Audio, Files, Video};
    has_blob || !matches!(kind, Files | Video | Audio)
}

/// Send one clip to a peer: a compressed metadata frame, then (if present) its
/// blob streamed as chunks. Used by both the live send path and the reconnect
/// replay so the wire format lives in one place.
async fn send_clip_frames<S>(
    write: &mut S,
    key: &Key,
    clip: &Clip,
    blob_dir: &std::path::Path,
    stream_blob: bool,
) -> Result<(), String>
where
    S: futures_util::Sink<Message> + Unpin,
{
    use base64::{engine::general_purpose::STANDARD, Engine};

    // Metadata frame: gzip before encrypt (text/code/links shrink 60-80%).
    // `send` flushes immediately — the common small-clip path stays low-latency.
    if let Ok(json) = serde_json::to_vec(clip) {
        write
            .send(Message::Binary(encrypt(key, &pack(&json, true))))
            .await
            .map_err(|_| "send failed".to_string())?;
    }

    // Blob content: stream as chunks (only when asked — see `stream_blob`). Skip
    // gzip; blobs are images/video/files that are already compressed.
    if !stream_blob {
        return Ok(());
    }
    if let Some(ref_) = clip.blob_ref.as_deref() {
        if let Some(path) = crate::blobs::existing_path(blob_dir, ref_) {
            if let Ok(bytes) = std::fs::read(&path) {
                let n = bytes.len().div_ceil(crate::proto::CHUNK).max(1);
                let mut in_window = 0usize;
                for (i, part) in bytes.chunks(crate::proto::CHUNK).enumerate() {
                    let frame = serde_json::json!({
                        "blob_chunk": ref_, "i": i, "n": n, "b64": STANDARD.encode(part),
                    });
                    let Ok(frame_bytes) = serde_json::to_vec(&frame) else {
                        continue;
                    };
                    let ct = encrypt(key, &pack(&frame_bytes, false));
                    write
                        .feed(Message::Binary(ct))
                        .await
                        .map_err(|_| "send failed".to_string())?;
                    in_window += 1;
                    if in_window >= SEND_WINDOW {
                        write.flush().await.map_err(|_| "send failed".to_string())?;
                        in_window = 0;
                    }
                }
                if in_window > 0 {
                    write.flush().await.map_err(|_| "send failed".to_string())?;
                }
            }
        }
    }
    Ok(())
}

/// Whether a clip's blob *content* should be streamed now: only the first time
/// we send a given blob ref on a live link. `streamed` is the bounded set of
/// refs already sent — so a reconnect replay re-streams a blob only if it was
/// never delivered (captured while offline), not on every reconnect.
fn should_stream_blob(clip: &Clip, streamed: &mut crate::proto::Dedup) -> bool {
    match clip.blob_ref.as_deref() {
        Some(r) => streamed.insert_new(r),
        None => false,
    }
}

fn spawn_sync(ctx: Ctx, cfg: SyncConfig) {
    tokio::spawn(async move {
        let endpoint = format!(
            "{}?account={}&device={}",
            cfg.url,
            url_encode(&cfg.account),
            url_encode(&cfg.device)
        );
        // Exponential reconnect backoff (1s → 30s) so a down/misconfigured relay
        // isn't hammered every few seconds (wasted CPU/battery + log spam). A
        // connection that stayed up a while resets the backoff to stay snappy.
        use std::time::{Duration, Instant};
        let mut backoff = Duration::from_secs(1);
        // Blob refs we've already streamed at least once on a live connection.
        // Persists across reconnects so the catch-up replay re-sends a blob's
        // *content* only when it was never sent (captured while offline) — not
        // on every reconnect (which would saturate the uplink). Bounded.
        let mut streamed = crate::proto::Dedup::new(4096);
        loop {
            let started = Instant::now();
            if let Err(e) = sync_connection(&ctx, &cfg, &endpoint, &mut streamed).await {
                eprintln!("carryall: sync ended: {e}");
            }
            if started.elapsed() >= Duration::from_secs(10) {
                backoff = Duration::from_secs(1); // was genuinely connected → reset
            }
            tokio::time::sleep(backoff).await;
            backoff = backoff.saturating_mul(2).min(Duration::from_secs(30));
        }
    });
}

async fn sync_connection(
    ctx: &Ctx,
    cfg: &SyncConfig,
    endpoint: &str,
    streamed: &mut crate::proto::Dedup,
) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::collections::HashMap;

    let (ws, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("carryall: sync connected");
    let (mut write, mut read) = ws.split();
    // Subscribe BEFORE the replay so no clip captured during catch-up is missed.
    let mut outgoing = ctx.outgoing.subscribe();

    // Catch-up: re-offer our recent clips so anything captured while the link
    // was down (and dropped from the bounded broadcast channel) still
    // propagates. The receiver dedups and ignores clips older than its start, so
    // replay is idempotent and never clobbers a clipboard.
    let replay: Vec<Clip> = ctx
        .store
        .lock()
        .ok()
        .and_then(|s| s.recent(REPLAY_ON_CONNECT).ok())
        .unwrap_or_default();
    // Replay METADATA ONLY (no blob chunks): catch-up should be small and fast,
    // not a re-upload of every recent image/video. Live copies still stream
    // their blobs (see the live arm below, which uses `streamed`); a peer
    // missing old media re-fetches it on the next copy.
    for clip in replay.into_iter().rev() {
        if is_syncable(clip.kind, clip.blob_ref.is_some()) {
            send_clip_frames(&mut write, &cfg.key, &clip, &ctx.blob_dir, false).await?;
        }
    }

    // Clips waiting on their blob to finish transferring, keyed by blob_ref.
    let mut pending: HashMap<String, Clip> = HashMap::new();
    // Blob reassembly (shared, unit-tested logic).
    let mut chunks = crate::proto::Reassembler::new();

    loop {
        tokio::select! {
            local = outgoing.recv() => match local {
                Ok(clip) => {
                    let stream_blob = should_stream_blob(&clip, streamed);
                    send_clip_frames(&mut write, &cfg.key, &clip, &ctx.blob_dir, stream_blob).await?;
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
            inbound = read.next() => match inbound {
                Some(Ok(Message::Binary(data))) => {
                    let Ok(packed) = decrypt(&cfg.key, &data) else { continue };
                    let plain = unpack(&packed);
                    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&plain) else { continue };

                    // A delete event — remove the clip from history (cross-device delete).
                    if let Some(id) = val.get("delete").and_then(|v| v.as_str()) {
                        if let Ok(store) = ctx.store.lock() {
                            let _ = store.delete(id, now_ms());
                        }
                        continue;
                    }

                    // A pin toggle — sync pinned state across devices.
                    if let Some(id) = val.get("pin").and_then(|v| v.as_str()) {
                        let pinned = val.get("pinned").and_then(|v| v.as_bool()).unwrap_or(true);
                        if let Ok(store) = ctx.store.lock() {
                            let _ = store.set_pinned(id, pinned);
                        }
                        continue;
                    }

                    if let Some(ref_) = val.get("blob_chunk").and_then(|v| v.as_str()) {
                        // Already have this blob on disk and not mid-transfer → ignore
                        // backlog re-sends (the `pending` check first avoids a stat on
                        // the normal path, where the ref IS pending).
                        if !pending.contains_key(ref_)
                            && crate::blobs::existing_path(&ctx.blob_dir, ref_).is_some()
                        {
                            continue;
                        }
                        // A blob chunk — accumulate, then finalize when complete.
                        let i = val.get("i").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let n = val.get("n").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                        let Some(b64) = val.get("b64").and_then(|v| v.as_str()) else { continue };
                        let Ok(bytes) = STANDARD.decode(b64) else { continue };
                        if let Some(whole) = chunks.add(ref_, i, n, bytes) {
                            // Integrity: drop a blob whose content doesn't match its ref.
                            if let Some(k) = pending.get(ref_).map(|c| c.kind) {
                                if !crate::blobs::verify_ref(k, ref_, &whole) {
                                    eprintln!("carryall: blob integrity check failed ({ref_}); dropping");
                                    pending.remove(ref_);
                                    continue;
                                }
                            }
                            crate::blobs::write(&ctx.blob_dir, ref_, &whole);
                            if let Some(clip) = pending.remove(ref_) {
                                finalize_apply(ctx, clip);
                            }
                        }
                        continue;
                    }

                    // Otherwise it's a clip metadata frame.
                    let Ok(clip) = serde_json::from_value::<Clip>(val) else { continue };
                    if let Ok(store) = ctx.store.lock() {
                        let _ = store.upsert(&clip);
                    }
                    match clip.blob_ref.as_deref() {
                        // Needs a blob we don't have yet — wait for its chunks.
                        Some(r) if crate::blobs::existing_path(&ctx.blob_dir, r).is_none() => {
                            crate::proto::pending_insert(&mut pending, r.to_string(), clip);
                        }
                        // No blob, or blob already present — apply immediately.
                        _ => finalize_apply(ctx, clip),
                    }
                }
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(e)) => return Err(e.to_string()),
                _ => {}
            },
        }
    }
}

/// Materialize (for files) and write a received clip to the native clipboard,
/// suppressing our own monitor from re-capturing the write.
///
/// Only *genuinely new* clips (created at/after daemon start) are written to the
/// clipboard, and each only once per session — so a relay backlog replay on
/// reconnect refills history without clobbering your current clipboard, and a
/// clip arriving via multiple paths is never applied twice.
pub(crate) fn finalize_apply(ctx: &Ctx, mut clip: Clip) {
    use clip_core::ContentKind;

    {
        let mut applied = ctx.applied.lock().unwrap();
        if !should_apply(clip.created_at, ctx.started, &clip.id, &mut applied) {
            return; // backlog/historical or already-applied — don't touch clipboard
        }
    }

    // For file-like clips, turn the synced blob into a real local file and point
    // the clip at it so it pastes as an actual file.
    if matches!(
        clip.kind,
        ContentKind::Files | ContentKind::Video | ContentKind::Audio
    ) {
        if let Some(ref_) = clip.blob_ref.clone() {
            let paths = crate::blobs::materialize_archive(&ctx.blob_dir, &ref_);
            if !paths.is_empty() {
                let locals: Vec<String> = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                clip.body = Some(locals.join("\n").into_bytes());
                // Predict the id our monitor will derive from these paths so the
                // suppress-guard matches and we don't re-broadcast.
                let uris: Vec<String> = locals.iter().map(|l| format!("file://{l}")).collect();
                if let Some(nc) = crate::capture::files_clip(uris) {
                    *ctx.suppress.lock().unwrap() = Some(nc.dedup_id());
                }
            }
        }
    } else {
        *ctx.suppress.lock().unwrap() = Some(clip.id.clone());
    }
    crate::apply::to_native_clipboard(&clip, &ctx.blob_dir);
}

/// Decide whether an incoming clip should be written to the native clipboard.
///
/// Pure so the anti-clobber / anti-double-apply rules are unit-tested:
/// - clips older than the daemon start (`started`) are history/backlog replays —
///   stored already, but must NOT overwrite the current clipboard;
/// - each clip id applies at most once per session (dedup across relay + LAN).
///
/// Records the id in `applied` when it returns true.
fn should_apply(
    created_at: i64,
    started: i64,
    id: &str,
    applied: &mut crate::proto::Dedup,
) -> bool {
    if created_at < started {
        return false;
    }
    applied.insert_new(id) // false if a recent duplicate
}

#[cfg(test)]
mod tests {
    use super::{account_salt, should_apply, url_encode};
    use crate::proto::Dedup;

    #[test]
    fn account_salt_pads_bytes_not_chars() {
        // ASCII: padded to 16 bytes with '#'
        assert_eq!(account_salt("yk"), b"yk##############".to_vec());
        // already >=16 bytes: unchanged
        assert_eq!(
            account_salt("0123456789abcdef"),
            b"0123456789abcdef".to_vec()
        );
        // non-ASCII: "café" = 5 UTF-8 bytes (é=0xC3 0xA9) → padded to 16 BYTES,
        // which is what the PWA must match (pad bytes, not 4 JS chars).
        let mut expected = "café".as_bytes().to_vec(); // 5 bytes
        while expected.len() < 16 {
            expected.push(b'#');
        }
        assert_eq!(account_salt("café"), expected);
        assert_eq!(account_salt("café").len(), 16);
    }

    #[test]
    fn url_encode_matches_what_the_relay_decodes() {
        // unreserved chars pass through
        assert_eq!(url_encode("yk-clip_1.0~"), "yk-clip_1.0~");
        // the chars that would otherwise break the query / pick a different room
        assert_eq!(url_encode("my team"), "my%20team");
        assert_eq!(url_encode("a&b"), "a%26b");
        assert_eq!(url_encode("a+b"), "a%2Bb");
        assert_eq!(url_encode("a#b"), "a%23b");
        assert_eq!(url_encode("100%"), "100%25");
    }

    #[test]
    fn applies_new_clip_once_then_dedups() {
        let mut seen = Dedup::new(64);
        let started = 1000;
        // new clip (created after start) → applies
        assert!(should_apply(1500, started, "a", &mut seen));
        // same id again → deduped
        assert!(!should_apply(1500, started, "a", &mut seen));
    }

    #[test]
    fn skips_backlog_clips_older_than_start() {
        let mut seen = Dedup::new(64);
        let started = 1000;
        // historical/backlog clip → not applied (no clipboard clobber)
        assert!(!should_apply(50, started, "old", &mut seen));
        // and it wasn't recorded as applied, so a later live copy still works
        assert!(should_apply(2000, started, "new", &mut seen));
    }

    #[test]
    fn clip_exactly_at_start_applies() {
        let mut seen = Dedup::new(64);
        assert!(should_apply(1000, 1000, "x", &mut seen));
    }
}
