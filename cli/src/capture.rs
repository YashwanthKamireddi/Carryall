//! Turn whatever is on the OS clipboard into a [`NewClip`]. Shared builders for
//! both backends (clipboard-rs for X11/Win/macOS, wl-clipboard for Wayland).

use clip_core::{classify_text, ContentKind, NewClip};
use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext, ContentFormat, RustImageData};

const TEXT_PREVIEW_MAX: usize = 400;

pub fn text_clip(text: &str) -> Option<NewClip> {
    if text.trim().is_empty() {
        return None;
    }
    let kind = classify_text(text).content_kind();
    let size = text.len() as u64;
    Some(NewClip {
        kind,
        preview: truncate(text, TEXT_PREVIEW_MAX),
        body: Some(text.as_bytes().to_vec()),
        blob_ref: None,
        size,
        id: None,
    })
}

pub fn image_clip(png: Vec<u8>) -> Option<NewClip> {
    if png.is_empty() {
        return None;
    }
    let preview = match RustImageData::from_bytes(&png).ok().map(|i| i.get_size()) {
        Some((w, h)) => format!("Image · {w}×{h}"),
        None => "Image".to_string(),
    };
    let id = clip_core::content_hash(ContentKind::Image, &png);
    let size = png.len() as u64;
    Some(NewClip {
        kind: ContentKind::Image,
        preview,
        body: Some(png),
        blob_ref: Some(id.clone()),
        size,
        id: Some(id),
    })
}

/// Largest image FILE we inline as an Image clip (thumbnail + full sync). Bigger
/// images stay generic file copies. Covers any realistic wallpaper/screenshot.
const MAX_INLINE_IMAGE_BYTES: u64 = 48 * 1024 * 1024;

/// A single copied image file → an Image clip (so a wallpaper/screenshot copied
/// in a file manager shows a preview and lands in the Images filter, instead of a
/// generic "Files" row). Returns None if it's not a single decodable image file.
fn single_image_clip(path: &str) -> Option<NewClip> {
    let p = std::path::Path::new(path);
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    ) {
        return None; // formats RustImageData reliably decodes
    }
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_INLINE_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    // Only treat as an image if it actually decodes as one.
    if RustImageData::from_bytes(&bytes).is_err() {
        return None;
    }
    image_clip(bytes)
}

pub fn files_clip(files: Vec<String>) -> Option<NewClip> {
    let files: Vec<String> = files
        .into_iter()
        .map(|f| normalize_uri(&f))
        .filter(|f| !f.is_empty())
        .collect();
    if files.is_empty() {
        return None;
    }
    // A lone image file copied from a file manager arrives as a uri-list, not
    // pixels — recognize it and present it as an image (preview + Images filter).
    if files.len() == 1 {
        if let Some(img) = single_image_clip(&files[0]) {
            return Some(img);
        }
    }
    let kind = files_kind(&files);
    let preview = summarize_files(&files);
    let joined = files.join("\n");
    let size = joined.len() as u64;
    Some(NewClip {
        kind,
        preview,
        body: Some(joined.into_bytes()),
        blob_ref: None,
        size,
        id: None,
    })
}

/// Clipboard format markers that mean "don't store this" — password managers and
/// transient/one-shot copies. Matched case-insensitively as substrings of the
/// platform format names, so one list covers macOS (`org.nspasteboard.*`),
/// Windows (`ExcludeClipboardContentFromMonitorProcessing`) and X11/KDE hints.
/// This mirrors the Wayland path's skip, giving the same privacy on every OS.
fn is_sensitive_format(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "concealed",           // macOS org.nspasteboard.ConcealedType
        "transient",           // macOS org.nspasteboard.TransientType
        "passwordmanagerhint", // X11 / KDE
        "x-kde-passwordmanager",
        "excludeclipboardcontentfrommonitorprocessing", // Windows
    ];
    MARKERS.iter().any(|m| n.contains(m))
}

/// Capture via a clipboard-rs context (X11/Win/macOS). Priority: image → files → text.
pub fn from_clipboard_rs(ctx: &ClipboardContext) -> Option<NewClip> {
    // Don't capture password-manager / transient clips — same privacy as Wayland.
    if let Ok(formats) = ctx.available_formats() {
        if formats.iter().any(|f| is_sensitive_format(f)) {
            return None;
        }
    }
    if ctx.has(ContentFormat::Image) {
        if let Ok(img) = ctx.get_image() {
            if let Ok(png) = img.to_png() {
                if let Some(c) = image_clip(png.get_bytes().to_vec()) {
                    return Some(c);
                }
            }
        }
    }
    if ctx.has(ContentFormat::Files) {
        if let Ok(files) = ctx.get_files() {
            if let Some(c) = files_clip(files) {
                return Some(c);
            }
        }
    }
    if ctx.has(ContentFormat::Text) {
        if let Ok(text) = ctx.get_text() {
            if let Some(c) = text_clip(&text) {
                return Some(c);
            }
        }
    }
    None
}

fn files_kind(files: &[String]) -> ContentKind {
    let all = |exts: &[&str]| files.iter().all(|f| exts.iter().any(|e| has_ext(f, e)));
    const VIDEO: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "m4v"];
    const AUDIO: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "m4a"];
    if all(VIDEO) {
        ContentKind::Video
    } else if all(AUDIO) {
        ContentKind::Audio
    } else {
        ContentKind::Files
    }
}
fn has_ext(path: &str, ext: &str) -> bool {
    path.rsplit('.')
        .next()
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}
fn summarize_files(files: &[String]) -> String {
    if files.len() == 1 {
        basename(&files[0]).to_string()
    } else {
        format!("{} files", files.len())
    }
}
fn basename(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}
fn normalize_uri(s: &str) -> String {
    let s = s.trim();
    percent_decode(s.strip_prefix("file://").unwrap_or(s))
}
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::is_sensitive_format;

    #[test]
    fn detects_sensitive_clipboard_markers_cross_os() {
        for f in [
            "org.nspasteboard.ConcealedType",
            "org.nspasteboard.TransientType",
            "x-kde-passwordManagerHint",
            "application/x-kde-passwordManagerHint",
            "ExcludeClipboardContentFromMonitorProcessing",
        ] {
            assert!(is_sensitive_format(f), "should skip: {f}");
        }
        for f in [
            "text/plain",
            "public.utf8-plain-text",
            "image/png",
            "CF_TEXT",
        ] {
            assert!(!is_sensitive_format(f), "should keep: {f}");
        }
    }
}
