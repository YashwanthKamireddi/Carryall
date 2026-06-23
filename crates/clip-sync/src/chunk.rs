//! Content-addressed chunking for large blobs (files, video, big images).
//!
//! Splitting a payload into hashed fixed-size chunks gives us three things the
//! "just use AirDrop" approach lacks: **resumable** transfer (re-send only the
//! missing chunks), **dedup** (a chunk already present anywhere is never sent
//! again), and **integrity** (each chunk is verified by its hash on arrival).

/// Chunk size. 256 KiB balances per-chunk overhead against resumability
/// granularity.
pub const CHUNK_SIZE: usize = 256 * 1024;

/// Split `data` into `(hash, bytes)` chunks. The hash is the chunk's blake3
/// digest and doubles as its content address in storage/transfer.
pub fn chunk(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    data.chunks(CHUNK_SIZE)
        .map(|c| (blake3::hash(c).to_hex().to_string(), c.to_vec()))
        .collect()
}

/// Just the ordered chunk hashes — the manifest a receiver diffs against what
/// it already has to decide which chunks to request.
pub fn chunk_hashes(data: &[u8]) -> Vec<String> {
    data.chunks(CHUNK_SIZE)
        .map(|c| blake3::hash(c).to_hex().to_string())
        .collect()
}

/// Reassemble chunks (in order) into the original payload.
pub fn reassemble(chunks: &[Vec<u8>]) -> Vec<u8> {
    chunks.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_reassembles() {
        let data: Vec<u8> = (0..(CHUNK_SIZE * 2 + 123)).map(|i| i as u8).collect();
        let chunks = chunk(&data);
        assert_eq!(chunks.len(), 3); // two full + one partial
        let bytes: Vec<Vec<u8>> = chunks.into_iter().map(|(_, b)| b).collect();
        assert_eq!(reassemble(&bytes), data);
    }

    #[test]
    fn identical_chunks_share_a_hash() {
        // A payload of all-zero bytes spanning two chunks: both full chunks are
        // identical, so dedup would store/transfer them once.
        let data = vec![0u8; CHUNK_SIZE * 2];
        let hashes = chunk_hashes(&data);
        assert_eq!(hashes[0], hashes[1]);
    }

    #[test]
    fn different_content_different_hash() {
        assert_ne!(chunk_hashes(b"alpha"), chunk_hashes(b"beta"));
    }

    #[test]
    fn empty_input() {
        assert!(chunk(b"").is_empty());
        assert!(chunk_hashes(b"").is_empty());
    }
}
