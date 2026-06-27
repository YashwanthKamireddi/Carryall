//! Content-addressed blob storage. Images keep a full copy on disk + an inline
//! thumbnail; files/videos/audio store the real file bytes by content hash so
//! the actual content (not just a path) can be transferred to other devices.

use std::fs;
use std::path::{Path, PathBuf};

use clip_core::{content_hash, ContentKind, NewClip};
use clipboard_rs::{common::RustImage, RustImageData};

const THUMB_MAX: u32 = 400;
/// Max single-file size we content-sync — sized to carry full-quality photos and
/// most videos without downscaling. The blob is content-addressed + integrity-
/// checked and always transferred byte-for-byte (no compression). NOTE: a received
/// blob is reassembled in memory, so this cap also bounds peak RAM (~2× on the
/// receiver); the mobile browser is the tightest client, so this stays well under
/// a level that could OOM a phone tab. Lifting further wants streaming-to-disk.
pub const MAX_BLOB_BYTES: u64 = 512 * 1024 * 1024;
/// Max total for a multi-file selection / folder archive. 2 GiB.
pub const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Prepare a freshly captured clip for storage + sync:
/// - Image: write full PNG to the blob store, keep a thumbnail inline.
/// - Files/Video/Audio (single existing file ≤ cap): store the real bytes,
///   keyed by content hash, so the content can be synced. `body` keeps the path
///   for local paste; `preview` keeps the filename.
pub fn externalize(dir: &Path, nc: NewClip) -> NewClip {
    match nc.kind {
        ContentKind::Image => externalize_image(dir, nc),
        ContentKind::Files | ContentKind::Video | ContentKind::Audio => stage_file(dir, nc),
        _ => nc,
    }
}

fn externalize_image(dir: &Path, mut nc: NewClip) -> NewClip {
    let Some(full) = nc.body.take() else {
        return nc;
    };
    let id = nc.id.clone().unwrap_or_else(|| nc.dedup_id());
    let path = png_path(dir, &id);
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

/// Stage one or more selected files/folders into a single content-addressed
/// archive blob, so a multi-file selection syncs as real files. Folders are
/// walked recursively; each entry's name is its path relative to the selection
/// (forward-slashed) so the receiver can rebuild the structure. `body` keeps the
/// original paths for local paste.
fn stage_file(dir: &Path, mut nc: NewClip) -> NewClip {
    let paths: Vec<String> = nc
        .body
        .as_ref()
        .map(|b| {
            String::from_utf8_lossy(b)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total: u64 = 0;
    for p in &paths {
        let path = Path::new(p);
        // The selection's own basename is the entry-name prefix (so a folder
        // keeps its name and structure on the other side).
        let base = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        if collect_into_archive(path, &base, &mut entries, &mut total).is_err() {
            return nc; // selection too big to content-sync
        }
    }
    if entries.is_empty() {
        return nc; // nothing content-syncable (e.g. empty folders only)
    }

    let archive = encode_archive(&entries);
    let id = content_hash(nc.kind, &archive);
    let path = bin_path(dir, &id);
    if !path.exists() {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(&path, &archive);
    }
    nc.blob_ref = Some(id);
    nc.size = total;
    nc
}

/// Recursively add `path` (a file or directory) to `entries`, naming each file
/// by `rel` (its path relative to the selection root, forward-slashed). Returns
/// `Err` only if the running `total` would exceed `MAX_ARCHIVE_BYTES`; oversized
/// individual files and unreadable entries are skipped, not fatal.
fn collect_into_archive(
    path: &Path,
    rel: &str,
    entries: &mut Vec<(String, Vec<u8>)>,
    total: &mut u64,
) -> Result<(), ()> {
    let Ok(meta) = fs::metadata(path) else {
        return Ok(()); // unreadable / broken symlink — skip
    };
    if meta.is_dir() {
        let Ok(rd) = fs::read_dir(path) else {
            return Ok(());
        };
        // Deterministic order so the same folder hashes to the same archive.
        let mut children: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            let name = child.file_name().map(|s| s.to_string_lossy().into_owned());
            let Some(name) = name else { continue };
            let child_rel = format!("{rel}/{name}");
            collect_into_archive(&child, &child_rel, entries, total)?;
        }
        return Ok(());
    }
    if !meta.is_file() || meta.len() > MAX_BLOB_BYTES {
        return Ok(()); // skip oversized individual files / non-regular entries
    }
    if *total + meta.len() > MAX_ARCHIVE_BYTES {
        return Err(()); // selection too big to content-sync
    }
    if let Ok(bytes) = fs::read(path) {
        *total += meta.len();
        entries.push((rel.to_string(), bytes));
    } // unreadable — skip
    Ok(())
}

// ---- multi-file archive container (LE, no external deps) ----
// magic "CLA1" | u32 count | [ u16 name_len | name | u64 data_len | data ]*

fn encode_archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"CLA1");
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, data) in entries {
        let nb = name.as_bytes();
        out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        out.extend_from_slice(nb);
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

fn decode_archive(b: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut p = 0usize;
    let take = |p: &mut usize, n: usize| -> Option<&[u8]> {
        let s = b.get(*p..*p + n)?;
        *p += n;
        Some(s)
    };
    if take(&mut p, 4)? != b"CLA1" {
        return None;
    }
    let count = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let nlen = u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?) as usize;
        let name = String::from_utf8_lossy(take(&mut p, nlen)?).into_owned();
        let dlen = u64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?) as usize;
        let data = take(&mut p, dlen)?.to_vec();
        out.push((name, data));
    }
    Some(out)
}

/// Read a blob's bytes by reference (tries file then legacy png).
pub fn read(dir: &Path, blob_ref: &str) -> Option<Vec<u8>> {
    fs::read(bin_path(dir, blob_ref))
        .or_else(|_| fs::read(png_path(dir, blob_ref)))
        .ok()
}

/// On-disk path for a blob if present (for transfer). None if missing.
pub fn existing_path(dir: &Path, blob_ref: &str) -> Option<PathBuf> {
    [bin_path(dir, blob_ref), png_path(dir, blob_ref)]
        .into_iter()
        .find(|p| p.exists())
}

/// Write received blob bytes into the store under `blob_ref`.
pub fn write(dir: &Path, blob_ref: &str, bytes: &[u8]) {
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(bin_path(dir, blob_ref), bytes);
}

/// Verify reassembled blob bytes against the advertised `blob_ref`.
///
/// Our refs are content-addressed (`content_hash(kind, bytes)` — 64 lowercase
/// hex). For those we recompute and compare, catching corruption or tampering.
/// Refs from other origins (e.g. the PWA's `f-<sha256>`) use a different scheme
/// we can't recompute here, so we accept them (the E2E AEAD already guards
/// transport integrity).
pub fn verify_ref(kind: ContentKind, blob_ref: &str, bytes: &[u8]) -> bool {
    let is_ours = blob_ref.len() == 64 && blob_ref.bytes().all(|b| b.is_ascii_hexdigit());
    if is_ours {
        content_hash(kind, bytes) == blob_ref
    } else {
        true
    }
}

/// Unpack a received archive blob into the received-files dir, returning the
/// real local paths (so a synced multi-file selection becomes real files).
pub fn materialize_archive(dir: &Path, blob_ref: &str) -> Vec<PathBuf> {
    let Some(bytes) = read(dir, blob_ref) else {
        return Vec::new();
    };
    let Some(entries) = decode_archive(&bytes) else {
        return Vec::new();
    };
    let recv = dir.join("received");
    let _ = fs::create_dir_all(&recv);
    let mut out = Vec::new();
    for (name, data) in entries {
        let Some(path) = safe_join(&recv, &name) else {
            continue; // name resolved outside recv (traversal) — skip
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent); // rebuild folder structure
        }
        if fs::write(&path, &data).is_ok() {
            out.push(path);
        }
    }
    out
}

/// Join an archive entry name under `base`, rebuilding sub-folders but rejecting
/// any path-traversal: absolute paths, `..`, drive prefixes, etc. Returns `None`
/// if nothing safe remains.
fn safe_join(base: &Path, name: &str) -> Option<PathBuf> {
    let mut out = base.to_path_buf();
    let mut added = false;
    for comp in name.split(['/', '\\']) {
        // Drop empty, current-dir, parent-dir, and any component with a drive/colon.
        if comp.is_empty() || comp == "." || comp == ".." || comp.contains(':') {
            continue;
        }
        out.push(comp);
        added = true;
    }
    if added {
        Some(out)
    } else {
        None
    }
}

fn png_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.png"))
}
fn bin_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.bin"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_roundtrip_multi_file() {
        let entries = vec![
            ("a.txt".to_string(), b"hello".to_vec()),
            ("doc.pdf".to_string(), vec![0u8, 1, 2, 255, 10, 13]),
            ("empty".to_string(), Vec::new()),
        ];
        let encoded = encode_archive(&entries);
        let decoded = decode_archive(&encoded).expect("decode");
        assert_eq!(decoded, entries);
    }

    #[test]
    fn verify_ref_catches_tampering_and_accepts_foreign() {
        let bytes = b"the real file content";
        let good = content_hash(ContentKind::Files, bytes); // our 64-hex scheme
        assert!(verify_ref(ContentKind::Files, &good, bytes)); // matches → ok
        assert!(!verify_ref(ContentKind::Files, &good, b"tampered")); // mismatch → fail
                                                                      // wrong kind also fails (kind is mixed into the hash)
        assert!(!verify_ref(ContentKind::Image, &good, bytes));
        // foreign ref scheme (e.g. PWA "f-<sha256>") is accepted (unverifiable)
        assert!(verify_ref(ContentKind::Files, "f-deadbeef", bytes));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_archive(b"not an archive").is_none());
        assert!(decode_archive(b"").is_none());
    }

    #[test]
    fn materialize_unpacks_all_files() {
        let dir = std::env::temp_dir().join(format!("clip-blob-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let entries = vec![
            ("one.txt".to_string(), b"first".to_vec()),
            ("two.pdf".to_string(), b"second".to_vec()),
        ];
        let archive = encode_archive(&entries);
        let id = content_hash(ContentKind::Files, &archive);
        write(&dir, &id, &archive);

        let paths = materialize_archive(&dir, &id);
        assert_eq!(paths.len(), 2);
        let names: Vec<_> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"one.txt".to_string()));
        assert!(names.contains(&"two.pdf".to_string()));
        assert_eq!(fs::read(dir.join("received/one.txt")).unwrap(), b"first");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_packs_recursively_and_rebuilds_structure() {
        let dir = std::env::temp_dir().join(format!("clip-folder-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Build a source folder: proj/{readme.txt, src/main.rs}
        let proj = dir.join("proj");
        fs::create_dir_all(proj.join("src")).unwrap();
        fs::write(proj.join("readme.txt"), b"hello").unwrap();
        fs::write(proj.join("src/main.rs"), b"fn main(){}").unwrap();

        // Stage the folder (as the clipboard would deliver its path in body).
        let nc = NewClip {
            kind: ContentKind::Files,
            preview: "proj".into(),
            body: Some(proj.to_string_lossy().as_bytes().to_vec()),
            blob_ref: None,
            size: 0,
            id: None,
        };
        let blob_root = dir.join("blobs");
        let staged = stage_file(&blob_root, nc);
        let blob_ref = staged.blob_ref.expect("folder should produce an archive");

        // Materialize on the "receiver" and confirm the structure is rebuilt.
        let paths = materialize_archive(&blob_root, &blob_ref);
        assert_eq!(paths.len(), 2, "both files unpacked");
        assert_eq!(
            fs::read(blob_root.join("received/proj/readme.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(
            fs::read(blob_root.join("received/proj/src/main.rs")).unwrap(),
            b"fn main(){}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_join_blocks_path_traversal() {
        let base = Path::new("/tmp/recv");
        // Traversal / absolute attempts resolve safely under base or are rejected.
        assert_eq!(
            safe_join(base, "../../etc/passwd"),
            Some(base.join("etc/passwd"))
        );
        assert_eq!(
            safe_join(base, "/etc/passwd"),
            Some(base.join("etc/passwd"))
        );
        assert_eq!(safe_join(base, "a/../../b"), Some(base.join("a/b")));
        assert_eq!(
            safe_join(base, "ok/sub/file.txt"),
            Some(base.join("ok/sub/file.txt"))
        );
        assert_eq!(
            safe_join(base, "C:\\Windows\\x"),
            Some(base.join("Windows/x"))
        );
        assert_eq!(safe_join(base, "../.."), None); // nothing safe remains
                                                    // critically, no result ever escapes base
        for n in [
            "../../etc/passwd",
            "/etc/passwd",
            "a/../../b",
            "C:\\Windows\\x",
        ] {
            assert!(safe_join(base, n).unwrap().starts_with(base));
        }
    }
}
