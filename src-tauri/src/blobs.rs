//! Content-addressed blob storage for large clip payloads (images now; video
//! and big files later). This keeps SQLite rows and IPC payloads small: the
//! full-resolution image lives on disk at `<blob_dir>/<id>.png`, while the row
//! keeps only a downscaled thumbnail inline for the grid.

use std::fs;
use std::path::{Path, PathBuf};

use clip_core::{ContentKind, NewClip};
use clipboard_rs::{common::RustImage, RustImageData};

/// Longest edge of the inline thumbnail.
const THUMB_MAX: u32 = 400;

/// If `nc` is an image carrying full bytes inline, persist those bytes to the
/// blob store and replace `body` with a thumbnail. Non-image kinds pass through
/// unchanged. Idempotent: an already-stored blob is not rewritten.
pub fn externalize(dir: &Path, mut nc: NewClip) -> NewClip {
    if nc.kind != ContentKind::Image {
        return nc;
    }
    let Some(full) = nc.body.take() else {
        return nc;
    };
    let id = nc.id.clone().unwrap_or_else(|| nc.dedup_id());

    let path = blob_path(dir, &id);
    if !path.exists() {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(&path, &full);
    }

    let thumb = RustImageData::from_bytes(&full)
        .ok()
        .and_then(|i| i.thumbnail(THUMB_MAX, THUMB_MAX).ok())
        .and_then(|t| t.to_png().ok())
        .map(|b| b.get_bytes().to_vec())
        .unwrap_or(full); // fall back to inlining the full image

    nc.body = Some(thumb);
    nc.blob_ref = Some(id);
    nc
}

/// Read a stored blob by its content-hash reference.
pub fn read(dir: &Path, blob_ref: &str) -> Option<Vec<u8>> {
    fs::read(blob_path(dir, blob_ref)).ok()
}

fn blob_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.png"))
}
