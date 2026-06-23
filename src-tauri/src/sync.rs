//! Desktop sync client (relay path).
//!
//! When configured, connects a WebSocket to the relay, end-to-end encrypts every
//! locally-captured clip and sends it, and applies clips received from the
//! account's other devices into the local store. The relay only ever sees
//! ciphertext (see `clip-sync::crypto`).
//!
//! Configuration is via environment for now (a settings UI + QR pairing comes
//! later); sync is **off unless `CLIP_ACCOUNT` is set**, so default runs are
//! fully local and private:
//! - `CLIP_ACCOUNT`   — room id shared by your devices (required to enable sync)
//! - `CLIP_SECRET`    — passphrase for the E2E key (defaults to `CLIP_ACCOUNT`)
//! - `CLIP_DEVICE`    — this device's id (defaults to `device-<pid>`)
//! - `CLIP_RELAY_URL` — ws URL (defaults to `ws://127.0.0.1:8787/sync`)

use std::time::Duration;

use clip_core::Clip;
use clip_sync::{decrypt, derive_key, encrypt, Key};
use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use crate::commands::to_dto;
use crate::state::AppState;

pub struct SyncConfig {
    pub account: String,
    pub device: String,
    pub url: String,
    pub key: Key,
}

/// Build sync config from the environment, or `None` if sync is disabled.
pub fn config_from_env() -> Option<SyncConfig> {
    let account = std::env::var("CLIP_ACCOUNT")
        .ok()
        .filter(|s| !s.is_empty())?;
    let secret = std::env::var("CLIP_SECRET").unwrap_or_else(|_| account.clone());
    let device =
        std::env::var("CLIP_DEVICE").unwrap_or_else(|_| format!("device-{}", std::process::id()));
    let url = std::env::var("CLIP_RELAY_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787/sync".into());

    // Argon2 needs a salt of >= 8 bytes; derive a stable one from the account.
    let mut salt = account.as_bytes().to_vec();
    while salt.len() < 16 {
        salt.push(b'#');
    }
    let key = derive_key(secret.as_bytes(), &salt).ok()?;

    Some(SyncConfig {
        account,
        device,
        url,
        key,
    })
}

/// Start the sync client on Tauri's async runtime. Reconnects with backoff.
pub fn spawn(app: AppHandle, cfg: SyncConfig) {
    tauri::async_runtime::spawn(async move {
        let endpoint = format!("{}?account={}&device={}", cfg.url, cfg.account, cfg.device);
        eprintln!(
            "clip: sync enabled (account={}, device={})",
            cfg.account, cfg.device
        );
        loop {
            match run_connection(&app, &cfg, &endpoint).await {
                Ok(()) => {}
                Err(e) => eprintln!("clip: sync connection ended: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(3)).await; // backoff before retry
        }
    });
}

async fn run_connection(app: &AppHandle, cfg: &SyncConfig, endpoint: &str) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("clip: sync connected to relay");
    let (mut write, mut read) = ws.split();

    // Subscribe to locally-captured clips to forward them.
    let mut outgoing = app.state::<AppState>().outgoing.subscribe();

    loop {
        tokio::select! {
            local = outgoing.recv() => match local {
                Ok(clip) => {
                    if let Ok(json) = serde_json::to_vec(&clip) {
                        let ct = encrypt(&cfg.key, &json);
                        if write.send(Message::Binary(ct)).await.is_err() {
                            return Err("send failed".into());
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => {} // dropped some; peers reconcile
                Err(RecvError::Closed) => return Ok(()),
            },
            inbound = read.next() => match inbound {
                Some(Ok(Message::Binary(data))) => apply_remote(app, cfg, &data),
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(e)) => return Err(e.to_string()),
                _ => {}
            },
        }
    }
}

/// Decrypt, deserialize and store a clip received from another device.
fn apply_remote(app: &AppHandle, cfg: &SyncConfig, data: &[u8]) {
    let Ok(plain) = decrypt(&cfg.key, data) else {
        eprintln!("clip: sync decrypt failed (wrong key or tampered)");
        return;
    };
    let Ok(clip) = serde_json::from_slice::<Clip>(&plain) else {
        return;
    };
    let state = app.state::<AppState>();
    {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Err(e) = store.upsert(&clip) {
            eprintln!("clip: sync upsert failed: {e}");
            return;
        }
    }
    let _ = app.emit("clip:new", to_dto(&clip));
}
