//! Pluggable per-account backlog of encrypted frames, replayed to devices on
//! connect for offline catch-up.
//!
//! - [`MemoryBacklog`]: in-process, lost on restart (the default).
//! - [`SqliteBacklog`]: persisted to disk, so a relay restart never drops the
//!   clips an offline device hasn't fetched yet.
//!
//! Both store only ciphertext (the relay never decrypts), so persistence adds
//! no plaintext exposure.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::relay::Frame;

/// A bounded, per-account store of recent frames.
pub trait BacklogStore: Send + Sync {
    /// Append a frame to an account's backlog, pruning to the capacity.
    fn append(&self, account: &str, frame: &Frame);
    /// Return an account's retained frames, oldest-first.
    fn recent(&self, account: &str) -> Vec<Frame>;
}

/// In-memory backlog: a per-account deque bounded by TOTAL BYTES.
///
/// Bounding by bytes (not frame count) is essential: frames range from ~200 B
/// (a text clip) to ~350 KB (one base64'd blob chunk), so a frame-count cap let
/// the backlog balloon to 100+ MB of old media chunks — which the relay then
/// dumped at every reconnecting device, flooding phones into a reconnect loop.
/// A byte budget keeps reconnect catch-up small and fast: recent clips replay;
/// large media syncs live (both online) rather than re-streaming from history.
pub struct MemoryBacklog {
    max_bytes: usize,
    rooms: Mutex<HashMap<String, VecDeque<Frame>>>,
}

impl MemoryBacklog {
    /// `max_bytes`: per-account byte budget for retained frames.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            rooms: Mutex::new(HashMap::new()),
        }
    }
}

impl BacklogStore for MemoryBacklog {
    fn append(&self, account: &str, frame: &Frame) {
        let mut rooms = self.rooms.lock().unwrap();
        let q = rooms.entry(account.to_string()).or_default();
        q.push_back(frame.clone());
        let mut total: usize = q.iter().map(|f| f.data.len()).sum();
        // Evict oldest until under budget, but always keep at least one frame.
        while q.len() > 1 && total > self.max_bytes {
            if let Some(f) = q.pop_front() {
                total -= f.data.len();
            }
        }
    }

    fn recent(&self, account: &str) -> Vec<Frame> {
        self.rooms
            .lock()
            .unwrap()
            .get(account)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Backstop against an unauthenticated relay accruing unbounded rooms (anyone
/// can connect with any account string): cap total retained frames across all
/// accounts, evicting the globally-oldest. Far above a personal relay's needs.
const GLOBAL_FRAME_CAP: i64 = 100_000;

/// SQLite-backed backlog: survives relay restarts. Bounded per-account by TOTAL
/// BYTES (see [`MemoryBacklog`] for why bytes, not frame count).
pub struct SqliteBacklog {
    max_bytes: usize,
    conn: Mutex<Connection>,
}

impl SqliteBacklog {
    /// `max_bytes`: per-account byte budget for retained frames.
    pub fn open(path: &str, max_bytes: usize) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            // `busy_timeout` lets a second process (e.g. a restart overlap) wait
            // for the writer instead of failing and silently dropping a frame.
            r#"PRAGMA journal_mode = WAL;
               PRAGMA busy_timeout = 5000;
               CREATE TABLE IF NOT EXISTS frames (
                   id          INTEGER PRIMARY KEY AUTOINCREMENT,
                   account     TEXT NOT NULL,
                   from_device TEXT NOT NULL,
                   data        BLOB NOT NULL
               );
               CREATE INDEX IF NOT EXISTS frames_account ON frames(account, id);"#,
        )?;
        Ok(Self {
            max_bytes,
            conn: Mutex::new(conn),
        })
    }
}

impl BacklogStore for SqliteBacklog {
    fn append(&self, account: &str, frame: &Frame) {
        let conn = self.conn.lock().unwrap();
        if conn
            .execute(
                "INSERT INTO frames (account, from_device, data) VALUES (?1, ?2, ?3)",
                params![account, frame.from, frame.data],
            )
            .is_err()
        {
            return;
        }
        // Prune to the most recent `max_bytes` for this account: delete any frame
        // whose newer-frames-byte-total already reached the budget (the newest
        // frame has 0 bytes ahead of it, so at least one is always kept).
        let _ = conn.execute(
            "DELETE FROM frames WHERE account = ?1 AND id IN (\
               SELECT id FROM (\
                 SELECT id, SUM(LENGTH(data)) OVER (ORDER BY id DESC) - LENGTH(data) AS before \
                 FROM frames WHERE account = ?1\
               ) WHERE before >= ?2)",
            params![account, self.max_bytes as i64],
        );
        // Global backstop so an unbounded number of rooms can't fill the disk.
        let _ = conn.execute(
            "DELETE FROM frames WHERE id NOT IN \
             (SELECT id FROM frames ORDER BY id DESC LIMIT ?1)",
            params![GLOBAL_FRAME_CAP],
        );
    }

    fn recent(&self, account: &str) -> Vec<Frame> {
        let conn = self.conn.lock().unwrap();
        // Return the newest frames within the byte budget, oldest-first.
        let mut stmt = match conn.prepare(
            "SELECT from_device, data FROM (\
               SELECT id, from_device, data, \
                 SUM(LENGTH(data)) OVER (ORDER BY id DESC) - LENGTH(data) AS before \
               FROM frames WHERE account = ?1\
             ) WHERE before < ?2 ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(params![account, self.max_bytes as i64], |r| {
            Ok(Frame {
                from: r.get(0)?,
                data: r.get(1)?,
            })
        });
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(from: &str, d: &[u8]) -> Frame {
        Frame {
            from: from.into(),
            data: d.to_vec(),
        }
    }

    #[test]
    fn memory_bounds_and_orders() {
        let b = MemoryBacklog::new(3); // 3-byte budget; frames here are 1 byte
        for i in 0..5u8 {
            b.append("a", &f("d", &[i]));
        }
        let got: Vec<_> = b.recent("a").iter().map(|x| x.data[0]).collect();
        assert_eq!(got, vec![2, 3, 4]); // newest 3 bytes, oldest-first
        assert!(b.recent("missing").is_empty());
    }

    #[test]
    fn bounds_by_bytes_not_frame_count() {
        // 50 × 100 B = 5000 B into a 1000 B budget → only the newest ~10 survive,
        // proving a big-frame history can't balloon the backlog (the flood bug).
        for store in [
            Box::new(MemoryBacklog::new(1000)) as Box<dyn BacklogStore>,
            Box::new(SqliteBacklog::open(&format!("{}/b.db", tempdir()), 1000).unwrap()),
        ] {
            for i in 0..50u8 {
                store.append("a", &f("d", &[i; 100]));
            }
            let got = store.recent("a");
            let total: usize = got.iter().map(|x| x.data.len()).sum();
            assert!(total <= 1000, "within byte budget, got {total}");
            assert!(!got.is_empty());
            assert_eq!(got.last().unwrap().data[0], 49); // newest retained
        }
    }

    #[test]
    fn sqlite_persists_across_reopen() {
        let dir = tempdir();
        let path = format!("{dir}/relay.db");
        {
            let b = SqliteBacklog::open(&path, 100).unwrap();
            b.append("acct", &f("desktop", b"missed-1"));
            b.append("acct", &f("desktop", b"missed-2"));
        }
        // Simulate a relay restart: reopen the same file.
        let b2 = SqliteBacklog::open(&path, 100).unwrap();
        let got: Vec<_> = b2.recent("acct").iter().map(|x| x.data.clone()).collect();
        assert_eq!(got, vec![b"missed-1".to_vec(), b"missed-2".to_vec()]);
    }

    #[test]
    fn sqlite_bounds_per_account() {
        let dir = tempdir();
        let b = SqliteBacklog::open(&format!("{dir}/r.db"), 3).unwrap();
        for i in 0..6u8 {
            b.append("a", &f("d", &[i]));
        }
        b.append("other", &f("d", b"z"));
        let got: Vec<_> = b.recent("a").iter().map(|x| x.data[0]).collect();
        assert_eq!(got, vec![3, 4, 5]);
        assert_eq!(b.recent("other").len(), 1); // accounts independent
    }

    // Minimal unique temp dir without extra deps.
    fn tempdir() -> String {
        let p = std::env::temp_dir().join(format!("clip-relay-test-{}", unique()));
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().into_owned()
    }
    fn unique() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::process::id() as u64 * 1_000_000 + N.fetch_add(1, Ordering::Relaxed)
    }
}
