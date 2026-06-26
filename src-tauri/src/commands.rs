//! IPC surface exposed to the SolidJS frontend, plus the `Clip` → `ClipDto`
//! projection (the frontend never sees raw bytes except as a thumbnail data URL).

use base64::{engine::general_purpose::STANDARD, Engine};
use clip_core::{Clip, ContentKind};
use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext, RustImageData};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::state::AppState;

/// What the UI receives for each clip. Heavy/raw bytes are omitted; images carry
/// a ready-to-render PNG data URL in `thumb`.
#[derive(Serialize, Clone)]
pub struct ClipDto {
    pub id: String,
    pub kind: String,
    pub preview: String,
    pub size: u64,
    pub created_at: i64,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

pub fn to_dto(c: &Clip) -> ClipDto {
    let thumb = if c.kind == ContentKind::Image {
        c.body
            .as_ref()
            .map(|b| format!("data:image/png;base64,{}", STANDARD.encode(b)))
    } else {
        None
    };
    ClipDto {
        id: c.id.clone(),
        kind: c.kind.as_str().to_string(),
        preview: c.preview.clone(),
        size: c.size,
        created_at: c.created_at,
        pinned: c.pinned,
        thumb,
    }
}

const DEFAULT_LIMIT: i64 = 200;

#[tauri::command]
pub fn list_clips(state: State<AppState>, limit: Option<i64>) -> Result<Vec<ClipDto>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let clips = store
        .recent(limit.unwrap_or(DEFAULT_LIMIT))
        .map_err(|e| e.to_string())?;
    Ok(clips.iter().map(to_dto).collect())
}

#[tauri::command]
pub fn search_clips(
    state: State<AppState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<ClipDto>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let clips = store
        .search(&query, limit.unwrap_or(DEFAULT_LIMIT))
        .map_err(|e| e.to_string())?;
    Ok(clips.iter().map(to_dto).collect())
}

/// Write a stored clip back onto the OS clipboard, then hide the palette so the
/// user can immediately paste into the focused app.
#[tauri::command]
pub fn copy_clip(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    let clip = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        store.get(&id).map_err(|e| e.to_string())?
    };
    let Some(clip) = clip else {
        return Err("clip not found".into());
    };

    let ctx = ClipboardContext::new().map_err(|e| e.to_string())?;
    match clip.kind {
        ContentKind::Image => {
            // Prefer the full-resolution blob; fall back to the inline thumbnail
            // (older rows) so paste-back always works.
            let bytes = clip
                .blob_ref
                .as_deref()
                .and_then(|r| crate::blobs::read(&state.blob_dir, r))
                .or(clip.body)
                .ok_or("image data missing")?;
            let img = RustImageData::from_bytes(&bytes).map_err(|e| e.to_string())?;
            ctx.set_image(img).map_err(|e| e.to_string())?;
        }
        ContentKind::Files | ContentKind::Video | ContentKind::Audio => {
            // Put real files on the clipboard (text/uri-list) so the target app
            // receives files, not the path string. Falls back to text.
            let body = clip.body.clone().unwrap_or_default();
            let paths: Vec<String> = String::from_utf8_lossy(&body)
                .lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect();
            if !set_files_clipboard(&paths) {
                ctx.set_text(paths.join("\n")).map_err(|e| e.to_string())?;
            }
        }
        _ => {
            // text/link/color/code roundtrip as their UTF-8 body.
            let text = clip
                .body
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or(clip.preview);
            ctx.set_text(text).map_err(|e| e.to_string())?;
        }
    }

    hide_palette(app);
    // Paste it straight into the app that regains focus — instant, beating
    // native history UIs that only put it on the clipboard.
    crate::paste::paste_after_focus_return();
    Ok(())
}

/// Put a file list on the clipboard as `text/uri-list` (Wayland via `wl-copy`),
/// so pasting yields real files. Returns false if it couldn't (caller falls
/// back to text). X11 file-copy is a future addition.
fn set_files_clipboard(paths: &[String]) -> bool {
    #[cfg(target_os = "linux")]
    {
        let wayland = std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var("WAYLAND_DISPLAY")
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        if wayland && cmd_exists("wl-copy") && !paths.is_empty() {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let uris = paths
                .iter()
                .map(|p| {
                    if p.starts_with("file://") {
                        p.clone()
                    } else {
                        format!("file://{}", p.replace(' ', "%20"))
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            if let Ok(mut child) = Command::new("wl-copy")
                .args(["--type", "text/uri-list"])
                .stdin(Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(uris.as_bytes());
                }
                let _ = child.wait();
                return true;
            }
        }
    }
    let _ = paths;
    false
}

#[cfg(target_os = "linux")]
fn cmd_exists(bin: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tauri::command]
pub fn set_pin(state: State<AppState>, id: String, pinned: bool) -> Result<(), String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.set_pinned(&id, pinned).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_clip(state: State<AppState>, id: String) -> Result<(), String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_history(state: State<AppState>) -> Result<(), String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.clear().map_err(|e| e.to_string())
}

/// Hide the palette window (called by the UI on Escape / after copy).
#[tauri::command]
pub fn hide_palette(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}
