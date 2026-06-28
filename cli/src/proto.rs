//! Shared sync-protocol helpers used by both transports (relay + LAN), so the
//! logic is written and tested once.

use std::collections::{HashMap, HashSet, VecDeque};

/// Bounded set of recently-seen ids (FIFO eviction), so a long-running daemon's
/// apply-dedup memory stays constant instead of growing forever. Duplicates
/// only need to be caught within a short window (relay + LAN arriving close
/// together / backlog replay), so evicting the oldest ids is harmless.
pub struct Dedup {
    set: HashSet<String>,
    order: VecDeque<String>,
    cap: usize,
}

impl Dedup {
    pub fn new(cap: usize) -> Self {
        Self {
            set: HashSet::new(),
            order: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Record `id`; returns true if it was newly inserted (not a recent dup).
    pub fn insert_new(&mut self, id: &str) -> bool {
        if self.set.contains(id) {
            return false;
        }
        self.set.insert(id.to_string());
        self.order.push_back(id.to_string());
        if self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }

    #[cfg(test)]
    pub fn contains(&self, id: &str) -> bool {
        self.set.contains(id)
    }
}

/// Raw bytes per blob chunk before base64 (kept in sync across transports, and
/// matched to the PWA's 256 KiB). Larger chunks mean far fewer per-chunk
/// encrypt/serialize/send operations on big files; one chunk base64s to ~341 KiB
/// — comfortably under both the relay (8 MiB) and LAN (16 MiB) frame caps.
pub const CHUNK: usize = 256 * 1024;

/// Max clips held awaiting their blob. Normal in-flight is ~1; this only bounds
/// memory if a peer streams metadata whose blob never completes. The clip is
/// already persisted to history before it lands here, so evicting one merely
/// skips its auto-apply to the native clipboard — overload protection, not loss.
pub const MAX_PENDING: usize = 256;

/// Insert `key→val` into a "waiting on blob" map, bounding it to `MAX_PENDING`
/// (drops an existing entry when full). Only triggers under pathological
/// never-completing transfers; in normal use the map holds ~1 entry.
pub fn pending_insert<V>(map: &mut HashMap<String, V>, key: String, val: V) {
    if map.len() >= MAX_PENDING && !map.contains_key(&key) {
        if let Some(victim) = map.keys().next().cloned() {
            map.remove(&victim);
        }
    }
    map.insert(key, val);
}

/// Max number of partially-received blobs held at once. A blob is normally
/// completed within milliseconds of its first chunk, so real in-flight count is
/// tiny; this caps memory if a peer sends chunks that never complete (disconnect
/// mid-transfer, or a backlog replay of metadata whose chunks were evicted).
const MAX_INFLIGHT: usize = 64;

/// Reassembles blobs from out-of-order chunk frames, keyed by blob ref.
/// Tolerant of duplicate chunks and any arrival order; yields the full bytes
/// exactly once when the final missing piece arrives. Bounded to `MAX_INFLIGHT`
/// partial blobs (oldest-started evicted first) so a never-completing transfer
/// can't grow memory without limit.
#[derive(Default)]
pub struct Reassembler {
    bufs: HashMap<String, Buf>,
    order: VecDeque<String>,
    cap: usize,
}

struct Buf {
    parts: Vec<Option<Vec<u8>>>,
    have: usize,
}

impl Reassembler {
    pub fn new() -> Self {
        Self {
            bufs: HashMap::new(),
            order: VecDeque::new(),
            cap: MAX_INFLIGHT,
        }
    }

    #[cfg(test)]
    fn with_cap(cap: usize) -> Self {
        Self {
            bufs: HashMap::new(),
            order: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Add chunk `i` of `n` for `key`. Returns `Some(whole)` once all `n`
    /// chunks have arrived (and clears the buffer); `None` until then.
    pub fn add(&mut self, key: &str, i: usize, n: usize, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if n == 0 {
            return None;
        }
        // New blob: enforce the in-flight cap by evicting the oldest partial.
        if !self.bufs.contains_key(key) {
            while self.order.len() >= self.cap {
                if let Some(old) = self.order.pop_front() {
                    self.bufs.remove(&old);
                } else {
                    break;
                }
            }
            self.order.push_back(key.to_string());
        }
        let buf = self.bufs.entry(key.to_string()).or_insert_with(|| Buf {
            parts: vec![None; n],
            have: 0,
        });
        if i < buf.parts.len() && buf.parts[i].is_none() {
            buf.parts[i] = Some(bytes);
            buf.have += 1;
        }
        if buf.have == buf.parts.len() {
            let buf = self.bufs.remove(key).unwrap();
            self.order.retain(|k| k != key);
            Some(buf.parts.into_iter().flatten().flatten().collect())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_detects_dupes_then_bounds_memory() {
        let mut d = Dedup::new(3);
        assert!(d.insert_new("a")); // new
        assert!(!d.insert_new("a")); // recent dup
        assert!(d.insert_new("b"));
        assert!(d.insert_new("c"));
        assert!(d.insert_new("d")); // exceeds cap → evicts "a"
        assert!(!d.contains("a")); // oldest evicted
        assert!(d.contains("d") && d.contains("b") && d.contains("c"));
        // "a" can be inserted again (evicted), which is harmless
        assert!(d.insert_new("a"));
    }

    #[test]
    fn single_chunk_completes_immediately() {
        let mut r = Reassembler::new();
        assert_eq!(r.add("a", 0, 1, b"hello".to_vec()), Some(b"hello".to_vec()));
    }

    #[test]
    fn in_order_chunks() {
        let mut r = Reassembler::new();
        assert_eq!(r.add("a", 0, 2, b"foo".to_vec()), None);
        assert_eq!(r.add("a", 1, 2, b"bar".to_vec()), Some(b"foobar".to_vec()));
    }

    #[test]
    fn out_of_order_chunks_reassemble_correctly() {
        let mut r = Reassembler::new();
        assert_eq!(r.add("a", 2, 3, b"baz".to_vec()), None);
        assert_eq!(r.add("a", 0, 3, b"foo".to_vec()), None);
        assert_eq!(
            r.add("a", 1, 3, b"bar".to_vec()),
            Some(b"foobarbaz".to_vec())
        );
    }

    #[test]
    fn duplicate_chunk_is_ignored() {
        let mut r = Reassembler::new();
        assert_eq!(r.add("a", 0, 2, b"foo".to_vec()), None);
        assert_eq!(r.add("a", 0, 2, b"DUP".to_vec()), None); // dup, not counted
        assert_eq!(r.add("a", 1, 2, b"bar".to_vec()), Some(b"foobar".to_vec()));
    }

    #[test]
    fn independent_keys() {
        let mut r = Reassembler::new();
        assert_eq!(r.add("a", 0, 1, b"A".to_vec()), Some(b"A".to_vec()));
        assert_eq!(r.add("b", 0, 1, b"B".to_vec()), Some(b"B".to_vec()));
    }

    #[test]
    fn pending_insert_bounds_the_map() {
        let mut m: HashMap<String, u32> = HashMap::new();
        for i in 0..(MAX_PENDING + 50) {
            pending_insert(&mut m, format!("k{i}"), i as u32);
        }
        assert!(m.len() <= MAX_PENDING, "pending map stays bounded");
        // Re-inserting an existing key doesn't evict (updates in place).
        let key = m.keys().next().cloned().unwrap();
        let len = m.len();
        pending_insert(&mut m, key, 999);
        assert_eq!(m.len(), len);
    }

    #[test]
    fn bounds_inflight_partials_evicting_oldest() {
        let mut r = Reassembler::with_cap(2);
        // Three never-completing 2-chunk blobs; cap is 2 in-flight.
        r.add("a", 0, 2, b"a0".to_vec()); // partial
        r.add("b", 0, 2, b"b0".to_vec()); // partial
        r.add("c", 0, 2, b"c0".to_vec()); // starting "c" evicts oldest ("a")
        assert_eq!(r.bufs.len(), 2);
        assert!(!r.bufs.contains_key("a"), "oldest partial evicted");
        assert!(r.bufs.contains_key("b") && r.bufs.contains_key("c"));
        // Completing a live one still works and frees its slot + order entry.
        assert_eq!(r.add("b", 1, 2, b"b1".to_vec()), Some(b"b0b1".to_vec()));
        assert_eq!(r.bufs.len(), 1);
        assert_eq!(r.order.len(), 1);
    }
}
