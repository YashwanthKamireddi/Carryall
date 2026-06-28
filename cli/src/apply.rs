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
            let uris: Vec<String> = body_text()
                .lines()
                .filter(|l| !l.is_empty())
                .map(|p| {
                    if p.starts_with("file://") {
                        p.to_string()
                    } else {
                        format!("file://{}", p.replace(' ', "%20"))
                    }
                })
                .collect();
            if is_wayland() {
                copy_files_wayland(&uris);
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

/// Put file URIs on the Wayland clipboard as a real FILE selection — offering
/// every target a file manager looks for, in one atomic offer, so pasting
/// copies the actual file(s) (not just the path text):
/// - `text/uri-list` — the generic file-reference standard;
/// - `x-special/gnome-copied-files` — GNOME/Nautilus (and GTK apps);
/// - `application/x-kde-cutselection` — KDE/Dolphin copy-vs-cut flag;
/// - `text/plain` — the local path, so text fields at least get something useful.
///
/// (Pasting a file as inline content into a *chat/text* app is app-specific on
/// Linux — most don't accept clipboard files — but file managers will now paste
/// the file itself. Images use `image/png` and paste inline everywhere.)
#[cfg(target_os = "linux")]
fn copy_files_wayland(uris: &[String]) {
    use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
    if uris.is_empty() {
        return;
    }
    let bytes = |s: String| Source::Bytes(s.into_bytes().into_boxed_slice());
    let mime = |m: &str| MimeType::Specific(m.to_string());
    let uri_list = format!("{}\r\n", uris.join("\r\n"));
    let gnome = format!("copy\n{}", uris.join("\n"));
    // Local paths (decoded from the file:// URIs) for the text/plain fallback.
    let plain = uris
        .iter()
        .map(|u| u.strip_prefix("file://").unwrap_or(u).replace("%20", " "))
        .collect::<Vec<_>>()
        .join("\n");
    let sources = vec![
        MimeSource {
            source: bytes(uri_list),
            mime_type: mime("text/uri-list"),
        },
        MimeSource {
            source: bytes(gnome),
            mime_type: mime("x-special/gnome-copied-files"),
        },
        MimeSource {
            source: bytes("0".to_string()),
            mime_type: mime("application/x-kde-cutselection"),
        },
        MimeSource {
            source: bytes(plain),
            mime_type: mime("text/plain;charset=utf-8"),
        },
    ];
    // Forks a background server to serve the selection, then returns. Best-effort;
    // fall back to a plain text/uri-list via the external tool if it fails.
    if Options::new().copy_multi(sources).is_err() {
        wl_copy(
            &["--type", "text/uri-list"],
            format!("{}\r\n", uris.join("\r\n")).as_bytes(),
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn copy_files_wayland(_uris: &[String]) {}

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
