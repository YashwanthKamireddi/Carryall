//! Turn whatever is on the OS clipboard into a [`NewClip`].
//!
//! The actual reading differs per platform/display server (clipboard-rs for
//! X11/Win/macOS, wl-clipboard for Wayland), so this module exposes small
//! **builder** functions that both backends share, plus an X11/native capture
//! path. Keeping the kind decisions here means every backend classifies
//! identically — the single choke point behind "capture *everything*".

use clip_core::{classify_text, ContentKind, NewClip};
use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext, ContentFormat, RustImageData};

/// Cap on inline text preview length (full text is still stored in `body`).
const TEXT_PREVIEW_MAX: usize = 400;

// ---------------------------------------------------------------------------
// Builders — shared by all backends.
// ---------------------------------------------------------------------------

/// Build a clip from text, refined into link/color/code/plain by clip-core.
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

/// Build an image clip from full-resolution PNG bytes.
///
/// Identity is the hash of the *full* image. The full PNG is carried in `body`
/// here; the blob layer ([`crate::blobs::externalize`]) then writes it to
/// content-addressed on-disk storage and swaps `body` for a small thumbnail.
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

/// Build a clip from a list of file paths or `file://` URIs.
pub fn files_clip(files: Vec<String>) -> Option<NewClip> {
    let files: Vec<String> = files
        .into_iter()
        .map(|f| normalize_uri(&f))
        .filter(|f| !f.is_empty())
        .collect();
    if files.is_empty() {
        return None;
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

// ---------------------------------------------------------------------------
// X11 / native capture via clipboard-rs.
// ---------------------------------------------------------------------------

/// Capture the current clipboard via a clipboard-rs context (X11/Win/macOS).
/// Priority: image → files → text.
pub fn from_clipboard_rs(ctx: &ClipboardContext) -> Option<NewClip> {
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

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Classify a file selection: all-video / all-audio / generic files.
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

/// Strip a leading `file://` and percent-decode a clipboard URI into a path.
fn normalize_uri(s: &str) -> String {
    let s = s.trim();
    let path = s.strip_prefix("file://").unwrap_or(s);
    percent_decode(path)
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
    use super::*;

    #[test]
    fn text_clip_classifies() {
        assert_eq!(text_clip("https://x.com").unwrap().kind, ContentKind::Link);
        assert_eq!(text_clip("#ff0066").unwrap().kind, ContentKind::Color);
        assert!(text_clip("   ").is_none());
    }

    #[test]
    fn video_selection_detected() {
        let files = vec!["/a/clip.mp4".to_string(), "/b/movie.MOV".to_string()];
        assert_eq!(files_kind(&files), ContentKind::Video);
    }

    #[test]
    fn mixed_selection_is_generic_files() {
        let files = vec!["/a/clip.mp4".to_string(), "/b/notes.txt".to_string()];
        assert_eq!(files_kind(&files), ContentKind::Files);
    }

    #[test]
    fn files_clip_normalizes_uris() {
        let c = files_clip(vec!["file:///home/me/my%20file.txt".into()]).unwrap();
        assert_eq!(c.preview, "my file.txt");
        assert_eq!(c.kind, ContentKind::Files);
    }

    #[test]
    fn summary_and_basename() {
        assert_eq!(summarize_files(&["/x/y/report.pdf".into()]), "report.pdf");
        assert_eq!(basename("/a/b/c.png"), "c.png");
    }

    #[test]
    fn truncate_respects_char_boundary() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
    }
}
