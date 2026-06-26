//! Background daemon: watches the native OS clipboard, stores every copy, and
//! (if configured) E2E-syncs it to the relay so it appears on your other
//! devices. Lightweight — no GUI, no WebView.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use clip_core::{Clip, NewClip};
use clip_store::Store;
use clip_sync::{decrypt, derive_key, encrypt, Key};
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
}

/// Data directory (kept compatible with prior history at com.yk.clip).
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.yk.clip")
}

impl Ctx {
    /// Persist a freshly captured clip (dedup + blob externalize) and forward to sync.
    fn ingest(&self, new_clip: NewClip) {
        let new_clip = crate::blobs::externalize(&self.blob_dir, new_clip);
        let now = now_ms();
        let stored = {
            let store = match self.store.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            match store.insert(new_clip, now) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("clip: store insert failed: {e}");
                    return;
                }
            }
        };
        let _ = self.outgoing.send(stored);
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
                eprintln!("clip daemon: already running (pid {pid}) — exiting");
                return;
            }
        }
    }
    let _ = std::fs::write(&pidfile, std::process::id().to_string());

    let store = match Store::open(dir.join("clip.db")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clip: cannot open store: {e}");
            return;
        }
    };
    let blob_dir = dir.join("blobs");
    let _ = std::fs::create_dir_all(&blob_dir);
    let (outgoing, _) = broadcast::channel(256);
    let ctx = Ctx {
        store: Arc::new(Mutex::new(store)),
        blob_dir,
        outgoing,
    };

    spawn_monitor(ctx.clone());
    eprintln!("clip daemon: capturing clipboard");

    if let Some(cfg) = SyncConfig::load() {
        eprintln!("clip daemon: sync enabled (account={})", cfg.account);
        spawn_sync(ctx.clone(), cfg);
    } else {
        eprintln!("clip daemon: local-only (set CLIP_ACCOUNT to enable sync)");
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
        self.last_id = Some(id);
        self.daemon.ingest(nc);
    }
}

fn clipboard_rs_loop(daemon: Ctx) {
    let ctx = match ClipboardContext::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("clip: clipboard init failed: {e}");
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
            eprintln!("clip: watcher init failed: {e}");
            return;
        }
    };
    watcher.add_handler(handler);
    watcher.start_watch();
}

#[cfg(target_os = "linux")]
fn wayland_loop(daemon: Ctx) {
    let mut last_id: Option<String> = None;
    loop {
        if let Some(nc) = read_wayland() {
            let id = nc.dedup_id();
            if last_id.as_deref() != Some(id.as_str()) {
                last_id = Some(id);
                daemon.ingest(nc);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
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
    capture::text_clip(&String::from_utf8_lossy(&read(MimeType::Text)?))
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
        let url = get("CLIP_RELAY_URL", "relay")
            .unwrap_or_else(|| "ws://127.0.0.1:8787/sync".into());
        let mut salt = account.as_bytes().to_vec();
        while salt.len() < 16 {
            salt.push(b'#');
        }
        let key = derive_key(secret.as_bytes(), &salt).ok()?;
        Some(SyncConfig { account, device, url, key })
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

fn spawn_sync(ctx: Ctx, cfg: SyncConfig) {
    tokio::spawn(async move {
        let endpoint = format!("{}?account={}&device={}", cfg.url, cfg.account, cfg.device);
        loop {
            if let Err(e) = sync_connection(&ctx, &cfg, &endpoint).await {
                eprintln!("clip: sync ended: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });
}

async fn sync_connection(ctx: &Ctx, cfg: &SyncConfig, endpoint: &str) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("clip: sync connected");
    let (mut write, mut read) = ws.split();
    let mut outgoing = ctx.outgoing.subscribe();
    loop {
        tokio::select! {
            local = outgoing.recv() => match local {
                Ok(clip) => {
                    if let Ok(json) = serde_json::to_vec(&clip) {
                        if write.send(Message::Binary(encrypt(&cfg.key, &json))).await.is_err() {
                            return Err("send failed".into());
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Ok(()),
            },
            inbound = read.next() => match inbound {
                Some(Ok(Message::Binary(data))) => {
                    if let Ok(plain) = decrypt(&cfg.key, &data) {
                        if let Ok(clip) = serde_json::from_slice::<Clip>(&plain) {
                            if let Ok(store) = ctx.store.lock() {
                                let _ = store.upsert(&clip);
                            }
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(e)) => return Err(e.to_string()),
                _ => {}
            },
        }
    }
}
