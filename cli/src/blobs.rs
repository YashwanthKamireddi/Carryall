//! Content-addressed blob storage for large image payloads: full PNG on disk,
//! thumbnail inline in the row.

use std::fs;
use std::path::{Path, PathBuf};

use clip_core::{ContentKind, NewClip};
use clipboard_rs::{common::RustImage, RustImageData};

const THUMB_MAX: u32 = 400;

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
        .unwrap_or(full);
    nc.body = Some(thumb);
    nc.blob_ref = Some(id);
    nc
}

pub fn read(dir: &Path, blob_ref: &str) -> Option<Vec<u8>> {
    fs::read(blob_path(dir, blob_ref)).ok()
}

fn blob_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.png"))
}
