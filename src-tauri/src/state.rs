//! Shared application state managed by Tauri and accessed from commands, the
//! clipboard monitor thread, and the sync client.

use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

use clip_core::Clip;
use clip_store::Store;
use tokio::sync::broadcast;

pub struct AppState {
    /// The local clip history. `Mutex` is fine here: clipboard events and UI
    /// queries are low-frequency relative to lock cost, and SQLite serializes
    /// writes anyway.
    pub store: Mutex<Store>,
    /// Directory holding content-addressed large blobs (full-res images, etc.).
    pub blob_dir: PathBuf,
    /// Unix-millis when the palette was last shown. Used to suppress the
    /// Spotlight hide-on-blur for a short grace period after showing, since
    /// Wayland fires a spurious unfocus right after a window maps.
    pub last_shown: Arc<AtomicI64>,
    /// Locally-captured clips are published here; the sync client (if enabled)
    /// subscribes and forwards them to the account's other devices. Broadcast
    /// so it works with or without a subscriber and survives reconnects.
    pub outgoing: broadcast::Sender<Clip>,
}

impl AppState {
    pub fn new(store: Store, blob_dir: PathBuf) -> Self {
        let (outgoing, _) = broadcast::channel(256);
        Self {
            store: Mutex::new(store),
            blob_dir,
            last_shown: Arc::new(AtomicI64::new(0)),
            outgoing,
        }
    }
}
