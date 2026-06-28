//! Write a clip onto the native OS clipboard. Used when a clip arrives from
//! another device — so copying on your phone lands in your desktop clipboard.
//!
//! On Wayland the native clipboard is the Wayland selection (what real apps and
//! `wl-paste` read), which is *separate* from the X11/XWayland clipboard that
//! clipboard-rs targets — so we must write via `wl-copy` there. On X11/Win/macOS
//! clipboard-rs is correct.

use std::io::Write;
use std::path::Path;

use clip_core::{Clip, ContentKind};
use clipboard_rs::{
    common::RustImage, Clipboard, ClipboardContent, ClipboardContext, RustImageData,
};

fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Put `clip` on the native clipboard. `blob_dir` is where image blobs live.
pub fn to_native_clipboard(clip: &Clip, blob_dir: &Path) {
    let body_text = || {
        clip.body
            .clone()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_else(|| clip.preview.clone())
    };

    match clip.kind {
        ContentKind::Image => {
            let bytes = clip
                .blob_ref
                .as_deref()
                .and_then(|r| crate::blobs::read(blob_dir, r))
                .or_else(|| clip.body.clone());
            if let Some(bytes) = bytes {
                if is_wayland() {
                    wl_copy(&["--type", "image/png"], &bytes);
                } else if let (Ok(ctx), Ok(img)) =
                    (ClipboardContext::new(), RustImageData::from_bytes(&bytes))
                {
                    let _ = ctx.set_image(img);
                }
            }
        }
        ContentKind::Files | ContentKind::Video | ContentKind::Audio => {
            let uris = body_text()
                .lines()
                .filter(|l| !l.is_empty())
                .map(|p| {
                    if p.starts_with("file://") {
                        p.to_string()
                    } else {
                        format!("file://{}", p.replace(' ', "%20"))
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            if is_wayland() {
                wl_copy(&["--type", "text/uri-list"], uris.as_bytes());
            } else if let Ok(ctx) = ClipboardContext::new() {
                let _ = ctx.set_text(body_text());
            }
        }
        _ => {
            let text = body_text();
            if is_wayland() {
                // wl-copy can't cleanly advertise text/html and text/plain in one
                // shot, so paste stays plain here (no regression).
                wl_copy(&[], text.as_bytes());
            } else if let Ok(ctx) = ClipboardContext::new() {
                match clip.html.clone() {
                    // Offer both HTML and plain so formatting survives in rich
                    // apps while plain-text targets still work.
                    Some(html) => {
                        let _ = ctx.set(vec![
                            ClipboardContent::Text(text),
                            ClipboardContent::Html(html),
                        ]);
                    }
                    None => {
                        let _ = ctx.set_text(text);
                    }
                }
            }
        }
    }
}

/// Pipe `data` to `wl-copy` with extra args. Best-effort.
fn wl_copy(args: &[&str], data: &[u8]) {
    use std::process::{Command, Stdio};
    if let Ok(mut child) = Command::new("wl-copy")
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data);
        }
        let _ = child.wait();
    }
}
