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

/// In-memory backlog: a bounded deque per account.
pub struct MemoryBacklog {
    cap: usize,
    rooms: Mutex<HashMap<String, VecDeque<Frame>>>,
}

impl MemoryBacklog {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            rooms: Mutex::new(HashMap::new()),
        }
    }
}

impl BacklogStore for MemoryBacklog {
    fn append(&self, account: &str, frame: &Frame) {
        let mut rooms = self.rooms.lock().unwrap();
        let q = rooms.entry(account.to_string()).or_default();
        q.push_back(frame.clone());
        while q.len() > self.cap {
            q.pop_front();
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

/// SQLite-backed backlog: survives relay restarts.
pub struct SqliteBacklog {
    cap: usize,
    conn: Mutex<Connection>,
}

impl SqliteBacklog {
    pub fn open(path: &str, cap: usize) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"PRAGMA journal_mode = WAL;
               CREATE TABLE IF NOT EXISTS frames (
                   id          INTEGER PRIMARY KEY AUTOINCREMENT,
                   account     TEXT NOT NULL,
                   from_device TEXT NOT NULL,
                   data        BLOB NOT NULL
               );
               CREATE INDEX IF NOT EXISTS frames_account ON frames(account, id);"#,
        )?;
        Ok(Self {
            cap,
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
        // Prune to the most recent `cap` frames for this account.
        let _ = conn.execute(
            "DELETE FROM frames WHERE account = ?1 AND id NOT IN \
             (SELECT id FROM frames WHERE account = ?1 ORDER BY id DESC LIMIT ?2)",
            params![account, self.cap as i64],
        );
    }

    fn recent(&self, account: &str) -> Vec<Frame> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT from_device, data FROM \
             (SELECT id, from_device, data FROM frames WHERE account = ?1 ORDER BY id DESC LIMIT ?2) \
             ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(params![account, self.cap as i64], |r| {
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
        let b = MemoryBacklog::new(3);
        for i in 0..5u8 {
            b.append("a", &f("d", &[i]));
        }
        let got: Vec<_> = b.recent("a").iter().map(|x| x.data[0]).collect();
        assert_eq!(got, vec![2, 3, 4]); // last 3, oldest-first
        assert!(b.recent("missing").is_empty());
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
