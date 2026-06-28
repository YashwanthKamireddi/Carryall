//! Local clip history backed by SQLite + FTS5.
//!
//! Kept free of any UI/platform dependency so it unit-tests in plain `cargo
//! test` (no WebView needed) and can be reused by the future mobile build.
//!
//! Design notes:
//! - Dedup is by content hash ([`NewClip::dedup_id`]). Re-copying the same
//!   content does not create a duplicate row; it bumps `created_at` so the item
//!   jumps back to the top of the history — matching what users expect.
//! - Full-text search uses an external-content FTS5 index over `preview`, kept
//!   in sync by triggers, so search stays sub-millisecond over large histories.
//! - Encryption at rest (SQLCipher) is a planned feature flag; the schema and
//!   API do not change when it is enabled.

use clip_core::{Clip, ContentKind, NewClip};
use rusqlite::{params, Connection, OptionalExtension};

pub type Result<T> = std::result::Result<T, rusqlite::Error>;

pub struct Store {
    conn: Connection,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS clips (
    rowid       INTEGER PRIMARY KEY,
    id          TEXT    NOT NULL UNIQUE,
    kind        TEXT    NOT NULL,
    preview     TEXT    NOT NULL,
    body        BLOB,
    blob_ref    TEXT,
    size        INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    pinned      INTEGER NOT NULL DEFAULT 0,
    html        TEXT
);

CREATE INDEX IF NOT EXISTS clips_created_at ON clips(created_at DESC);
CREATE INDEX IF NOT EXISTS clips_kind ON clips(kind);

-- Tombstones: ids of clips deleted on any device, with the deletion time, so a
-- backlog replay on reconnect (same old created_at) can't resurrect them — while
-- a genuine re-copy (newer created_at) is allowed back. Bounded (oldest pruned).
CREATE TABLE IF NOT EXISTS tombstones (
    id          TEXT PRIMARY KEY,
    deleted_at  INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS clips_fts
    USING fts5(preview, content='clips', content_rowid='rowid');

CREATE TRIGGER IF NOT EXISTS clips_ai AFTER INSERT ON clips BEGIN
    INSERT INTO clips_fts(rowid, preview) VALUES (new.rowid, new.preview);
END;
CREATE TRIGGER IF NOT EXISTS clips_ad AFTER DELETE ON clips BEGIN
    INSERT INTO clips_fts(clips_fts, rowid, preview) VALUES ('delete', old.rowid, old.preview);
END;
CREATE TRIGGER IF NOT EXISTS clips_au AFTER UPDATE ON clips BEGIN
    INSERT INTO clips_fts(clips_fts, rowid, preview) VALUES ('delete', old.rowid, old.preview);
    INSERT INTO clips_fts(rowid, preview) VALUES (new.rowid, new.preview);
END;
"#;

impl Store {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        // Migration: add `html` to databases created before rich-text fidelity.
        // (CREATE TABLE IF NOT EXISTS won't alter an existing table.) Idempotent.
        let has_html = conn
            .prepare("SELECT 1 FROM pragma_table_info('clips') WHERE name = 'html'")?
            .exists([])?;
        if !has_html {
            conn.execute("ALTER TABLE clips ADD COLUMN html TEXT", [])?;
        }
        Ok(Self { conn })
    }

    /// Insert a freshly captured clip, deduping by content hash. Returns the
    /// stored clip (with its assigned id). Re-inserting identical content bumps
    /// it to the top instead of creating a duplicate.
    pub fn insert(&self, clip: NewClip, now_ms: i64) -> Result<Clip> {
        let stored = clip.into_clip(now_ms);
        // A fresh local capture of this content is an intentional revival —
        // drop any tombstone so it's allowed back into history + sync.
        self.conn
            .execute("DELETE FROM tombstones WHERE id = ?1", params![stored.id])?;
        self.conn.execute(
            r#"INSERT INTO clips (id, kind, preview, body, blob_ref, size, created_at, pinned, html)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)
               ON CONFLICT(id) DO UPDATE SET created_at = excluded.created_at"#,
            params![
                stored.id,
                stored.kind.as_str(),
                stored.preview,
                stored.body,
                stored.blob_ref,
                stored.size as i64,
                stored.created_at,
                stored.html,
            ],
        )?;
        // Return the canonical stored row (pinned flag may differ from a prior copy).
        self.get(&stored.id).map(|c| c.unwrap_or(stored))
    }

    /// Insert or update a fully-formed clip (used when applying a clip received
    /// from another device). Preserves the incoming id/body/blob_ref; keeps the
    /// most recent `created_at` and the incoming `pinned` state.
    pub fn upsert(&self, c: &Clip) -> Result<()> {
        // A clip deleted on another device must not be resurrected by a backlog
        // replay (same, older created_at). But a genuine re-copy afterwards has a
        // newer created_at — that clears the tombstone and is allowed back.
        if let Some(deleted_at) = self.tombstoned_at(&c.id)? {
            if c.created_at <= deleted_at {
                return Ok(());
            }
            self.conn
                .execute("DELETE FROM tombstones WHERE id = ?1", params![c.id])?;
        }
        self.conn.execute(
            r#"INSERT INTO clips (id, kind, preview, body, blob_ref, size, created_at, pinned, html)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
               ON CONFLICT(id) DO UPDATE SET
                 created_at = MAX(created_at, excluded.created_at),
                 pinned = MAX(pinned, excluded.pinned),
                 html = COALESCE(excluded.html, html)"#,
            params![
                c.id,
                c.kind.as_str(),
                c.preview,
                c.body,
                c.blob_ref,
                c.size as i64,
                c.created_at,
                c.pinned as i64,
                c.html,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Clip>> {
        self.conn
            .query_row(
                "SELECT id, kind, preview, body, blob_ref, size, created_at, pinned, html \
                 FROM clips WHERE id = ?1",
                params![id],
                row_to_clip,
            )
            .optional()
    }

    /// Most-recent clips first, pinned items always on top.
    pub fn recent(&self, limit: i64) -> Result<Vec<Clip>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, preview, body, blob_ref, size, created_at, pinned, html \
             FROM clips ORDER BY pinned DESC, created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], row_to_clip)?;
        rows.collect()
    }

    /// Full-text search over previews. Falls back gracefully if the query has no
    /// usable terms. Pinned-first, then recency.
    pub fn search(&self, query: &str, limit: i64) -> Result<Vec<Clip>> {
        let fts = to_fts_query(query);
        if fts.is_empty() {
            return self.recent(limit);
        }
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.kind, c.preview, c.body, c.blob_ref, c.size, c.created_at, c.pinned, c.html \
             FROM clips_fts f JOIN clips c ON c.rowid = f.rowid \
             WHERE clips_fts MATCH ?1 \
             ORDER BY c.pinned DESC, c.created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts, limit], row_to_clip)?;
        rows.collect()
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE clips SET pinned = ?2 WHERE id = ?1",
            params![id, pinned as i64],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: &str, now_ms: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        // Record a tombstone (with deletion time) so a backlog replay can't bring
        // it back, then bound the table to the most recent deletions.
        self.conn.execute(
            "INSERT INTO tombstones (id, deleted_at) VALUES (?1, ?2) \
             ON CONFLICT(id) DO UPDATE SET deleted_at = excluded.deleted_at",
            params![id, now_ms],
        )?;
        self.conn.execute(
            "DELETE FROM tombstones WHERE id IN \
             (SELECT id FROM tombstones ORDER BY deleted_at DESC LIMIT -1 OFFSET 4096)",
            [],
        )?;
        Ok(())
    }

    /// The deletion time of `id` if it's tombstoned (and not since revived).
    pub fn tombstoned_at(&self, id: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT deleted_at FROM tombstones WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
    }

    /// Remove all non-pinned clips (used by "clear history").
    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM clips WHERE pinned = 0", [])?;
        Ok(())
    }

    /// Retention: delete non-pinned clips older than `cutoff_ms`. Returns count.
    pub fn prune_older_than(&self, cutoff_ms: i64) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM clips WHERE pinned = 0 AND created_at < ?1",
            params![cutoff_ms],
        )
    }

    pub fn count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM clips", [], |r| r.get(0))
    }
}

fn row_to_clip(row: &rusqlite::Row) -> rusqlite::Result<Clip> {
    let kind_str: String = row.get(1)?;
    let kind = ContentKind::from_token(&kind_str).unwrap_or(ContentKind::Binary);
    let size: i64 = row.get(5)?;
    let pinned: i64 = row.get(7)?;
    Ok(Clip {
        id: row.get(0)?,
        kind,
        preview: row.get(2)?,
        body: row.get(3)?,
        blob_ref: row.get(4)?,
        html: row.get(8)?,
        size: size as u64,
        created_at: row.get(6)?,
        pinned: pinned != 0,
    })
}

/// Turn arbitrary user input into a safe FTS5 prefix query: keep alphanumeric
/// tokens, append `*` for prefix matching, AND them together. This avoids FTS5
/// syntax errors from punctuation the user types in the search box.
fn to_fts_query(input: &str) -> String {
    input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t}*"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_clip(s: &str) -> NewClip {
        NewClip {
            kind: ContentKind::Text,
            preview: s.to_string(),
            body: Some(s.as_bytes().to_vec()),
            blob_ref: None,
            html: None,
            size: s.len() as u64,
            id: None,
        }
    }

    #[test]
    fn insert_and_fetch_recent() {
        let store = Store::open_in_memory().unwrap();
        store.insert(text_clip("hello world"), 1000).unwrap();
        store.insert(text_clip("second item"), 2000).unwrap();
        let recent = store.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].preview, "second item"); // newest first
    }

    #[test]
    fn html_roundtrips_through_store() {
        let store = Store::open_in_memory().unwrap();
        let mut nc = text_clip("bold text");
        nc.html = Some("<b>bold text</b>".into());
        let id = store.insert(nc, 1000).unwrap().id;
        assert_eq!(
            store.get(&id).unwrap().unwrap().html.as_deref(),
            Some("<b>bold text</b>")
        );
        assert_eq!(
            store.recent(10).unwrap()[0].html.as_deref(),
            Some("<b>bold text</b>")
        );
        // A plain clip keeps html None.
        let pid = store.insert(text_clip("plain"), 2000).unwrap().id;
        assert_eq!(store.get(&pid).unwrap().unwrap().html, None);
    }

    #[test]
    fn migrates_a_pre_html_database() {
        // Simulate an old DB: a `clips` table without the `html` column.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE clips (rowid INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, \
             kind TEXT NOT NULL, preview TEXT NOT NULL, body BLOB, blob_ref TEXT, \
             size INTEGER NOT NULL, created_at INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO clips (id, kind, preview, size, created_at) VALUES ('x','text','old',3,1)",
            [],
        )
        .unwrap();
        // init() must add the column and keep the existing row usable.
        let store = Store::init(conn).unwrap();
        let got = store.get("x").unwrap().unwrap();
        assert_eq!(got.preview, "old");
        assert_eq!(got.html, None);
        // And new html writes work post-migration.
        let mut nc = text_clip("rich");
        nc.html = Some("<i>rich</i>".into());
        let id = store.insert(nc, 9).unwrap().id;
        assert_eq!(
            store.get(&id).unwrap().unwrap().html.as_deref(),
            Some("<i>rich</i>")
        );
    }

    #[test]
    fn reinsert_dedups_and_bumps_to_top() {
        let store = Store::open_in_memory().unwrap();
        store.insert(text_clip("alpha"), 1000).unwrap();
        store.insert(text_clip("beta"), 2000).unwrap();
        store.insert(text_clip("alpha"), 3000).unwrap(); // same content again
        assert_eq!(store.count().unwrap(), 2); // no duplicate row
        let recent = store.recent(10).unwrap();
        assert_eq!(recent[0].preview, "alpha"); // bumped back to top
    }

    #[test]
    fn full_text_search() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(text_clip("the quick brown fox"), 1000)
            .unwrap();
        store.insert(text_clip("lazy dogs sleep"), 2000).unwrap();
        let hits = store.search("brown", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].preview, "the quick brown fox");
        // prefix match
        assert_eq!(store.search("qui", 10).unwrap().len(), 1);
        // punctuation in query must not crash FTS
        assert_eq!(store.search("brown!!! ()", 10).unwrap().len(), 1);
        // empty query returns recent
        assert_eq!(store.search("   ", 10).unwrap().len(), 2);
    }

    #[test]
    fn pin_keeps_on_top_and_survives_clear() {
        let store = Store::open_in_memory().unwrap();
        let pinned = store.insert(text_clip("keep me"), 1000).unwrap();
        store.insert(text_clip("newer"), 5000).unwrap();
        store.set_pinned(&pinned.id, true).unwrap();
        assert_eq!(store.recent(10).unwrap()[0].preview, "keep me");
        store.clear().unwrap();
        let left = store.recent(10).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].preview, "keep me");
    }

    #[test]
    fn delete_and_prune() {
        let store = Store::open_in_memory().unwrap();
        let a = store.insert(text_clip("old"), 1000).unwrap();
        store.insert(text_clip("new"), 9000).unwrap();
        store.delete(&a.id, 9000).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        store.insert(text_clip("ancient"), 500).unwrap();
        let removed = store.prune_older_than(8000).unwrap();
        assert_eq!(removed, 1); // "ancient" pruned, "new" kept
    }

    #[test]
    fn upsert_preserves_identity_and_keeps_newest_time() {
        let store = Store::open_in_memory().unwrap();
        let mut clip = store.insert(text_clip("synced"), 1000).unwrap();
        // Simulate the same clip arriving from another device, newer + pinned.
        clip.created_at = 5000;
        clip.pinned = true;
        store.upsert(&clip).unwrap();
        assert_eq!(store.count().unwrap(), 1); // same id ⇒ no duplicate
        let got = store.get(&clip.id).unwrap().unwrap();
        assert_eq!(got.created_at, 5000);
        assert!(got.pinned);
        // An older copy must not roll the timestamp back.
        clip.created_at = 2000;
        store.upsert(&clip).unwrap();
        assert_eq!(store.get(&clip.id).unwrap().unwrap().created_at, 5000);

        // A clip re-broadcast (e.g. backlog replay) with pinned=false must NOT
        // unpin it — unpinning only happens via the explicit set_pinned path.
        clip.pinned = false;
        store.upsert(&clip).unwrap();
        assert!(store.get(&clip.id).unwrap().unwrap().pinned);
        store.set_pinned(&clip.id, false).unwrap(); // explicit unpin still works
        assert!(!store.get(&clip.id).unwrap().unwrap().pinned);
    }

    #[test]
    fn tombstone_blocks_backlog_resurrection_but_recapture_revives() {
        let store = Store::open_in_memory().unwrap();
        let clip = store.insert(text_clip("secret"), 1000).unwrap();
        store.delete(&clip.id, 2000).unwrap(); // deleted at t=2000
        assert_eq!(store.tombstoned_at(&clip.id).unwrap(), Some(2000));

        // A backlog replay (same old created_at) must NOT bring it back.
        store.upsert(&clip).unwrap(); // clip.created_at == 1000 <= 2000
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.get(&clip.id).unwrap().is_none());

        // A genuine re-copy afterwards (newer created_at) clears the tombstone.
        let mut later = clip.clone();
        later.created_at = 3000;
        store.upsert(&later).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        assert!(store.tombstoned_at(&clip.id).unwrap().is_none());

        // And a fresh local capture always revives, regardless of timing.
        store.delete(&clip.id, 9000).unwrap();
        let revived = store.insert(text_clip("secret"), 4000).unwrap();
        assert_eq!(revived.id, clip.id);
        assert_eq!(store.count().unwrap(), 1);
        assert!(store.tombstoned_at(&clip.id).unwrap().is_none());
    }

    #[test]
    fn persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.db");
        {
            let store = Store::open(&path).unwrap();
            store.insert(text_clip("durable"), 1000).unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.recent(10).unwrap()[0].preview, "durable");
    }
}
