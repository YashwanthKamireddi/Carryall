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

/// Raw bytes per blob chunk before base64 (kept in sync across transports).
pub const CHUNK: usize = 48 * 1024;

/// Reassembles blobs from out-of-order chunk frames, keyed by blob ref.
/// Tolerant of duplicate chunks and any arrival order; yields the full bytes
/// exactly once when the final missing piece arrives.
#[derive(Default)]
pub struct Reassembler {
    bufs: HashMap<String, Buf>,
}

struct Buf {
    parts: Vec<Option<Vec<u8>>>,
    have: usize,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add chunk `i` of `n` for `key`. Returns `Some(whole)` once all `n`
    /// chunks have arrived (and clears the buffer); `None` until then.
    pub fn add(&mut self, key: &str, i: usize, n: usize, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if n == 0 {
            return None;
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
}
