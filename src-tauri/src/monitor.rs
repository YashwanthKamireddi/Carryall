//! Background clipboard monitor.
//!
//! Two backends, chosen at runtime:
//! - **X11 / Windows / macOS**: event-driven via clipboard-rs's watcher.
//! - **Wayland**: a poll loop over the wlr-data-control protocol
//!   (`wl-clipboard-rs`), because clipboard-rs only sees the X11/XWayland
//!   selection and would miss native Wayland copies.
//!
//! Both feed [`store_and_emit`], which dedups, persists, and pushes the new
//! clip to the UI.

use clip_core::NewClip;
use clipboard_rs::{ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext};
use tauri::{AppHandle, Emitter, Manager};

use crate::capture;
use crate::commands::to_dto;
use crate::state::AppState;

/// Persist a freshly captured clip and notify the UI. Shared by all backends.
fn store_and_emit(app: &AppHandle, new_clip: NewClip) {
    let state = app.state::<AppState>();
    // Move large payloads (images) to on-disk blob storage, leaving a
    // thumbnail inline, before persisting the row.
    let new_clip = crate::blobs::externalize(&state.blob_dir, new_clip);
    let now = crate::now_ms();
    let stored = {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("clip: store lock poisoned: {e}");
                return;
            }
        };
        match store.insert(new_clip, now) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("clip: store insert failed: {e}");
                return;
            }
        }
    };
    let _ = app.emit("clip:new", to_dto(&stored));
    // Forward to the sync client (if enabled) to push to other devices.
    let _ = state.outgoing.send(stored);
}

/// Choose and start the right backend for this session.
pub fn spawn(app: AppHandle) {
    #[cfg(target_os = "linux")]
    if is_wayland() {
        eprintln!("clip: using Wayland (wlr-data-control) clipboard backend");
        spawn_wayland(app);
        return;
    }
    spawn_clipboard_rs(app);
}

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

// --- clipboard-rs backend (X11 / Windows / macOS) --------------------------

struct ClipboardRsMonitor {
    ctx: ClipboardContext,
    app: AppHandle,
    last_id: Option<String>,
}

impl ClipboardHandler for ClipboardRsMonitor {
    fn on_clipboard_change(&mut self) {
        let Some(new_clip) = capture::from_clipboard_rs(&self.ctx) else {
            return;
        };
        let id = new_clip.dedup_id();
        if self.last_id.as_deref() == Some(id.as_str()) {
            return;
        }
        self.last_id = Some(id);
        store_and_emit(&self.app, new_clip);
    }
}

fn spawn_clipboard_rs(app: AppHandle) {
    std::thread::spawn(move || {
        let ctx = match ClipboardContext::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("clip: clipboard context init failed: {e}");
                return;
            }
        };
        let handler = ClipboardRsMonitor {
            ctx,
            app,
            last_id: None,
        };
        let mut watcher = match ClipboardWatcherContext::new() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("clip: clipboard watcher init failed: {e}");
                return;
            }
        };
        watcher.add_handler(handler);
        watcher.start_watch(); // blocks
    });
}

// --- Wayland backend (wlr-data-control) ------------------------------------

#[cfg(target_os = "linux")]
fn spawn_wayland(app: AppHandle) {
    use std::time::Duration;
    std::thread::spawn(move || {
        let mut last_id: Option<String> = None;
        loop {
            if let Some(new_clip) = read_wayland() {
                let id = new_clip.dedup_id();
                if last_id.as_deref() != Some(id.as_str()) {
                    last_id = Some(id);
                    store_and_emit(&app, new_clip);
                }
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
}

/// Read the current Wayland selection, preferring image → files → text.
#[cfg(target_os = "linux")]
fn read_wayland() -> Option<NewClip> {
    use std::io::Read;
    use wl_clipboard_rs::paste::{get_contents, get_mime_types, ClipboardType, MimeType, Seat};

    let types = get_mime_types(ClipboardType::Regular, Seat::Unspecified).ok()?;
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
        let files: Vec<String> = text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();
        return capture::files_clip(files);
    }
    let bytes = read(MimeType::Text)?;
    capture::text_clip(&String::from_utf8_lossy(&bytes))
}
