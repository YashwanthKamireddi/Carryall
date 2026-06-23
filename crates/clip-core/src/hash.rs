//! Content-addressed hashing. blake3 is used because it is extremely fast
//! (multi-GB/s, SIMD) — important since every copy is hashed on the hot path —
//! and the digest doubles as both the dedup key and the sync content address.

use crate::model::ContentKind;

/// Hash `payload` together with its `kind` and return a lowercase hex digest.
///
/// The kind is mixed in so that identical bytes interpreted as different kinds
/// (e.g. a string of digits as `Text` vs `Code`) do not collide.
pub fn content_hash(kind: ContentKind, payload: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update(&[0]); // domain separator between kind and payload
    hasher.update(payload);
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let a = content_hash(ContentKind::Text, b"abc");
        let b = content_hash(ContentKind::Text, b"abc");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // 32-byte digest in hex
    }

    #[test]
    fn kind_changes_digest() {
        assert_ne!(
            content_hash(ContentKind::Text, b"abc"),
            content_hash(ContentKind::Code, b"abc"),
        );
    }
}
