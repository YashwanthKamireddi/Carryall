//! Transparent compression applied to a plaintext *before* E2E encryption.
//!
//! Encrypted bytes are incompressible, so any size win has to come *before*
//! [`crate::encrypt`] — WebSocket-level `permessage-deflate` on ciphertext buys
//! nothing. Text/code/links (the common clipboard payloads) compress 60-80%, so
//! this is the single biggest transport speedup; already-compressed blobs
//! (images/video) are sent with compression skipped.
//!
//! Packed layout (this is what gets encrypted): `codec(1) || payload`.
//! - `0x00` — payload is the raw bytes.
//! - `0x01` — payload is gzip-compressed bytes.
//!
//! Any other leading byte is treated as untagged data and returned unchanged.
//! Every frame this codebase produced before the scheme existed is JSON (starts
//! with `{`, 0x7B), so those still decode; the fallback is not a general escape
//! for arbitrary bytes that happen to begin with 0x00/0x01. gzip (RFC 1952) is
//! used so the browser PWA can pack/unpack with the native
//! `CompressionStream`/`DecompressionStream` APIs — no JS dependency.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

const RAW: u8 = 0x00;
const GZIP: u8 = 0x01;

/// Hard ceiling on a single frame's decompressed size — defense against a
/// "gzip bomb". Frames are AEAD-encrypted so the relay can't forge one, but a
/// peer sharing the account key could send a tiny frame that inflates to
/// gigabytes and OOM-kills a daemon or the phone tab. This sits far above any
/// real payload (the relay caps a frame at 8 MiB *compressed*, and natural text
/// inflates only a few-fold), so it never truncates a legitimate clip.
const MAX_DECOMPRESSED: usize = 256 * 1024 * 1024;

/// Wrap `plaintext` with a codec byte, gzip-compressing when `try_gzip` is set
/// *and* it actually shrinks the data (tiny payloads gzip larger). The returned
/// buffer is what callers should encrypt.
pub fn pack(plaintext: &[u8], try_gzip: bool) -> Vec<u8> {
    if try_gzip {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        if enc.write_all(plaintext).is_ok() {
            if let Ok(gz) = enc.finish() {
                if gz.len() < plaintext.len() {
                    let mut out = Vec::with_capacity(gz.len() + 1);
                    out.push(GZIP);
                    out.extend_from_slice(&gz);
                    return out;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(plaintext.len() + 1);
    out.push(RAW);
    out.extend_from_slice(plaintext);
    out
}

/// Reverse [`pack`]: strip the codec byte and inflate if needed. Returns the
/// input unchanged when it carries no recognized codec tag (an older untagged
/// frame), so the scheme stays backward compatible. Returns an empty vec if a
/// gzip payload is corrupt — the caller's parse then simply skips that frame.
pub fn unpack(data: &[u8]) -> Vec<u8> {
    unpack_capped(data, MAX_DECOMPRESSED)
}

/// [`unpack`] with an explicit inflation ceiling (so the cap is unit-testable).
fn unpack_capped(data: &[u8], max: usize) -> Vec<u8> {
    match data.first() {
        Some(&GZIP) => {
            // Bound the reader: a gzip stream that inflates past `max` is treated
            // as hostile and dropped, rather than allocated in full.
            let mut out = Vec::new();
            let mut reader = GzDecoder::new(&data[1..]).take(max as u64 + 1);
            match reader.read_to_end(&mut out) {
                Ok(_) if out.len() <= max => out,
                _ => Vec::new(), // corrupt, or over the cap → drop the frame
            }
        }
        Some(&RAW) => data[1..].to_vec(),
        // Untagged legacy frame (JSON starting with `{`): pass through.
        _ => data.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_gzip_for_compressible_text() {
        let text = "fn main() { println!(\"hello\"); }\n".repeat(50);
        let packed = pack(text.as_bytes(), true);
        assert_eq!(packed[0], GZIP, "repetitive text should compress");
        assert!(packed.len() < text.len(), "and end up smaller");
        assert_eq!(unpack(&packed), text.as_bytes());
    }

    #[test]
    fn tiny_payload_stays_raw() {
        let packed = pack(b"hi", true);
        assert_eq!(packed[0], RAW, "gzip would be larger → keep raw");
        assert_eq!(unpack(&packed), b"hi");
    }

    #[test]
    fn skip_gzip_flag_keeps_raw() {
        let blob = vec![7u8; 4096];
        let packed = pack(&blob, false);
        assert_eq!(packed[0], RAW);
        assert_eq!(unpack(&packed), blob);
    }

    #[test]
    fn untagged_legacy_frame_passes_through() {
        // A pre-codec JSON frame starts with '{' (0x7B), neither RAW nor GZIP.
        let legacy = br#"{"id":"x"}"#;
        assert_eq!(unpack(legacy), legacy.to_vec());
    }

    #[test]
    fn corrupt_gzip_yields_empty() {
        let mut bad = vec![GZIP];
        bad.extend_from_slice(b"not actually gzip");
        assert!(unpack(&bad).is_empty());
    }

    #[test]
    fn binary_roundtrip() {
        let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        let packed = pack(&data, true);
        assert_eq!(unpack(&packed), data);
    }

    #[test]
    fn decompression_over_cap_is_rejected() {
        // A small gzip frame that inflates well past a tiny cap → dropped (empty),
        // never allocated in full. Models the gzip-bomb defense.
        let big = vec![0u8; 100_000];
        let packed = pack(&big, true);
        assert_eq!(packed[0], GZIP);
        assert!(
            unpack_capped(&packed, 1024).is_empty(),
            "over-cap → dropped"
        );
        // Under a generous cap it still round-trips.
        assert_eq!(unpack_capped(&packed, 1_000_000), big);
    }
}
