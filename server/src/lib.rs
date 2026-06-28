//! Clip sync relay, as a library so integration tests can drive a real server.
//! See [`router`] for the Axum app; the binary (`main.rs`) just serves it.

mod backlog;
mod bus;
pub mod relay;

use std::sync::Arc;

use backlog::SqliteBacklog;
use bus::RedisBus;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::header,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use relay::{Frame, Hub};

// The PWA (iPhone/web client) is embedded and served by the relay so a phone
// only needs the relay's URL — no separate web host.
const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
const SW_JS: &str = include_str!("../web/sw.js");
const ICON_SVG: &str = include_str!("../web/icon.svg");
const ICON_PNG: &[u8] = include_bytes!("../web/icon.png");

fn served(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, content_type)], body)
}

#[derive(Deserialize)]
struct Params {
    account: String,
    device: String,
}

/// Build the relay router with a fresh in-memory hub.
pub fn router() -> Router {
    router_with(Arc::new(Hub::default()))
}

/// Build the router from environment:
/// - `CLIP_RELAY_DB`  → durable SQLite backlog (offline catch-up survives restarts).
/// - `CLIP_REDIS_URL` → Redis bus for multi-instance HA (any instance, any account).
///
/// Both are optional; with neither set the relay is a single in-memory instance.
pub async fn router_from_env() -> Router {
    // A durable SQLite backlog is the DEFAULT so a relay restart never drops a
    // device's offline catch-up. The default path is relative to the working
    // directory (persistent — NOT the temp dir, which can be tmpfs or swept
    // mid-run); set `WorkingDirectory`/mount a volume there. Override with
    // `CLIP_RELAY_DB=<path>`, or set it to `memory` to opt out. A failed open
    // degrades to in-memory (logged) rather than refusing to start.
    fn open_backlog(path: &str) -> Hub {
        match SqliteBacklog::open(path, 500) {
            Ok(store) => {
                eprintln!("clip-relay: durable backlog at {path}");
                Hub::with_backlog(256, Box::new(store))
            }
            Err(e) => {
                eprintln!("clip-relay: backlog open failed ({e}); using in-memory");
                Hub::default()
            }
        }
    }
    let mut hub = match std::env::var("CLIP_RELAY_DB").ok().as_deref() {
        Some("memory") | Some("none") | Some("off") => {
            eprintln!("clip-relay: in-memory backlog (offline catch-up lost on restart)");
            Hub::default()
        }
        Some(path) if !path.is_empty() => open_backlog(path),
        _ => open_backlog("carryall-relay-backlog.db"),
    };

    let redis_url = std::env::var("CLIP_REDIS_URL")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(url) = &redis_url {
        match RedisBus::connect(url).await {
            Ok(bus) => {
                let origin = relay::gen_origin();
                hub.attach_bus(bus, origin.clone());
                let hub = Arc::new(hub);
                match bus::spawn_subscriber(url, hub.clone(), origin).await {
                    Ok(()) => eprintln!("clip-relay: Redis bus enabled ({url})"),
                    Err(e) => eprintln!("clip-relay: Redis subscriber failed: {e}"),
                }
                return router_with(hub);
            }
            Err(e) => eprintln!("clip-relay: Redis connect failed ({e}); single-instance"),
        }
    }
    router_with(Arc::new(hub))
}

/// Build the relay router over an existing hub (used by tests).
pub fn router_with(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route(
            "/app.js",
            get(|| async { served("text/javascript; charset=utf-8", APP_JS) }),
        )
        .route(
            "/manifest.webmanifest",
            get(|| async { served("application/manifest+json", MANIFEST) }),
        )
        .route(
            "/sw.js",
            get(|| async { served("text/javascript; charset=utf-8", SW_JS) }),
        )
        .route(
            "/icon.svg",
            get(|| async { served("image/svg+xml", ICON_SVG) }),
        )
        .route(
            "/icon.png",
            get(|| async { ([(header::CONTENT_TYPE, "image/png")], ICON_PNG) }),
        )
        .route("/health", get(|| async { "ok" }))
        .route("/sync", get(ws_handler))
        .with_state(hub)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(p): Query<Params>,
    State(hub): State<Arc<Hub>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| client_loop(socket, hub, p.account, p.device))
}

/// Reject single frames larger than this (defense against abuse). Large blobs
/// are chunked client-side well under this.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Frees an account's connection slot when the client loop exits, on any path.
struct ConnGuard {
    hub: Arc<Hub>,
    account: String,
}
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.hub.release(&self.account);
    }
}

/// Bridge one device's WebSocket to its account room: replay the backlog for
/// offline catch-up, then publish inbound messages and forward room frames from
/// *other* devices down the socket.
async fn client_loop(mut socket: WebSocket, hub: Arc<Hub>, account: String, device: String) {
    // Enforce the per-account connection cap; the guard frees the slot on exit.
    if !hub.try_acquire(&account) {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    let _guard = ConnGuard {
        hub: hub.clone(),
        account: account.clone(),
    };

    let (backlog, mut rx) = hub.join(&account);

    // Catch-up: send frames this device may have missed while offline.
    for frame in backlog {
        if frame.from != device && socket.send(Message::Binary(frame.data)).await.is_err() {
            return;
        }
    }

    // Keep the connection warm: a ping every 30s stops idle WebSockets (a phone
    // behind a Cloudflare Tunnel, cellular NAT, etc.) from being dropped, so the
    // next send is instant instead of paying a reconnect handshake. Clients
    // auto-respond with pong; no app-level traffic.
    // First tick after 30s (not immediately — a fresh connection needs no ping,
    // and an at-connect ping would race ahead of the first real frame).
    let mut keepalive = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(30),
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if socket.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Binary(data))) => {
                    if data.len() <= MAX_FRAME_BYTES {
                        hub.publish(&account, Frame { from: device.clone(), data });
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if text.len() <= MAX_FRAME_BYTES {
                        hub.publish(&account, Frame { from: device.clone(), data: text.into_bytes() });
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            },
            outbound = rx.recv() => match outbound {
                Ok(frame) if frame.from != device => {
                    if socket.send(Message::Binary(frame.data)).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
        }
    }
}
