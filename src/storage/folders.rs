//! Persistence for the directory-size tree.
//!
//! Lives beside `db.rs` rather than inside it: the metrics tables and the
//! folder tree share a connection but nothing else, and `db.rs` is already
//! long enough.
//!
//! Only directories at or above [`MIN_SIZE_BYTES`] are stored. On the author's
//! machine that is 4 558 rows out of 300 432 directories — everything dropped
//! is a folder that could never appear in a "largest folders" list.

use crate::collector::folder_sizes::FolderSize;
use crate::storage::db::MetricsDb;
use crate::sync::guard;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::path::Path;

/// Timestamp (Unix ms) of the last completed full scan.
const META_LAST_FULL_SCAN: &str = "folder_scan_completed_at";

/// One row as the API serves it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FolderRow {
    pub path: String,
    pub name: String,
    pub size_bytes: i64,
    pub file_count: i64,
    /// Unix ms when this row's number was last measured. Per-row, not
    /// per-scan: the on-open refresh updates some rows and not others, and
    /// the UI shows which is which.
    pub scanned_at: i64,
    /// Entries beneath this folder that could not be read. Non-zero means the
    /// size is a lower bound.
    pub unreadable: i64,
    /// Whether this folder has recorded children, so the UI knows if drilling
    /// in will show anything.
    pub has_children: bool,
}

impl MetricsDb {
    /// Create the folder-size schema. Called from `MetricsDb::open`.
    pub(super) fn init_folder_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folder_sizes (
                path        TEXT PRIMARY KEY,
                parent      TEXT NOT NULL,
                size_bytes  INTEGER NOT NULL,
                file_count  INTEGER NOT NULL,
                scanned_at  INTEGER NOT NULL,
                unreadable  INTEGER NOT NULL DEFAULT 0
            );
            -- Serves the only read query we make: a parent's children, largest
            -- first. Covering, so the lookup never touches the table.
            CREATE INDEX IF NOT EXISTS idx_folder_parent_size
                ON folder_sizes(parent, size_bytes DESC);",
        )
    }

    /// Replace the entire tree under `root` with the results of a full scan.
    ///
    /// One transaction, so readers see either the old tree or the new one and
    /// never a half-written mixture. A scan that dies partway leaves the
    /// previous generation intact.
    pub fn replace_folder_tree(
        &self,
        root: &Path,
        folders: &[FolderSize],
        scanned_at: i64,
    ) -> Result<usize, rusqlite::Error> {
        let mut conn = guard(self.conn.lock());
        let tx = conn.transaction()?;

        // Scope the delete to this root so a future multi-volume scan cannot
        // wipe another root's rows.
        let prefix = format!("{}/%", root.to_string_lossy());
        tx.execute(
            "DELETE FROM folder_sizes WHERE path = ?1 OR path LIKE ?2",
            params![root.to_string_lossy(), prefix],
        )?;

        let mut written = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO folder_sizes
                    (path, parent, size_bytes, file_count, scanned_at, unreadable)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for f in folders {
                stmt.execute(params![
                    f.path.to_string_lossy(),
                    f.parent.to_string_lossy(),
                    f.size_bytes as i64,
                    f.file_count as i64,
                    scanned_at,
                    f.unreadable as i64,
                ])?;
                written += 1;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![META_LAST_FULL_SCAN, scanned_at.to_string()],
        )?;

        tx.commit()?;
        Ok(written)
    }

    /// Update individual rows after an on-open verification pass.
    ///
    /// Unlike [`Self::replace_folder_tree`] this touches only the paths given
    /// and leaves the rest of the tree — including the last full scan's
    /// timestamp — alone.
    pub fn update_folder_rows(
        &self,
        folders: &[FolderSize],
        scanned_at: i64,
    ) -> Result<usize, rusqlite::Error> {
        let mut conn = guard(self.conn.lock());
        let tx = conn.transaction()?;
        let mut written = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO folder_sizes
                    (path, parent, size_bytes, file_count, scanned_at, unreadable)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for f in folders {
                stmt.execute(params![
                    f.path.to_string_lossy(),
                    f.parent.to_string_lossy(),
                    f.size_bytes as i64,
                    f.file_count as i64,
                    scanned_at,
                    f.unreadable as i64,
                ])?;
                written += 1;
            }
        }
        tx.commit()?;
        Ok(written)
    }

    /// The `limit` largest recorded children of `parent`, largest first.
    pub fn folder_children(
        &self,
        parent: &str,
        limit: u32,
    ) -> Result<Vec<FolderRow>, rusqlite::Error> {
        let conn = guard(self.conn.lock());
        let mut stmt = conn.prepare(
            "SELECT c.path, c.size_bytes, c.file_count, c.scanned_at, c.unreadable,
                    EXISTS(SELECT 1 FROM folder_sizes g WHERE g.parent = c.path)
             FROM folder_sizes c
             WHERE c.parent = ?1
             ORDER BY c.size_bytes DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![parent, limit], |r| {
            let path: String = r.get(0)?;
            Ok(FolderRow {
                name: leaf_name(&path),
                path,
                size_bytes: r.get(1)?,
                file_count: r.get(2)?,
                scanned_at: r.get(3)?,
                unreadable: r.get(4)?,
                has_children: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// A single folder's row, if it was recorded.
    pub fn folder_entry(&self, path: &str) -> Result<Option<FolderRow>, rusqlite::Error> {
        let conn = guard(self.conn.lock());
        conn.query_row(
            "SELECT c.path, c.size_bytes, c.file_count, c.scanned_at, c.unreadable,
                    EXISTS(SELECT 1 FROM folder_sizes g WHERE g.parent = c.path)
             FROM folder_sizes c WHERE c.path = ?1",
            params![path],
            |r| {
                let path: String = r.get(0)?;
                Ok(FolderRow {
                    name: leaf_name(&path),
                    path,
                    size_bytes: r.get(1)?,
                    file_count: r.get(2)?,
                    scanned_at: r.get(3)?,
                    unreadable: r.get(4)?,
                    has_children: r.get(5)?,
                })
            },
        )
        .optional()
    }

    /// When the last full scan finished, as Unix ms.
    pub fn last_full_scan_at(&self) -> Option<i64> {
        let conn = guard(self.conn.lock());
        conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_LAST_FULL_SCAN],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
    }
}

/// Final path component, or the whole path for a root like `/`.
fn leaf_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk_db() -> MetricsDb {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = format!("/tmp/webtop-folders-test-{}-{n}.db", std::process::id());
        let _ = std::fs::remove_file(&path);
        MetricsDb::open(&path).expect("open test db")
    }

    fn folder(path: &str, parent: &str, size: u64, files: u64) -> FolderSize {
        FolderSize {
            path: PathBuf::from(path),
            parent: PathBuf::from(parent),
            size_bytes: size,
            file_count: files,
            unreadable: 0,
        }
    }

    #[test]
    fn children_come_back_largest_first() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[
                folder("/home/small", "/home", 100, 1),
                folder("/home/big", "/home", 900, 3),
                folder("/home/mid", "/home", 500, 2),
            ],
            1_000,
        )
        .unwrap();

        let kids = db.folder_children("/home", 5).unwrap();

        let names: Vec<&str> = kids.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, vec!["big", "mid", "small"]);
    }

    #[test]
    fn limit_truncates_to_the_largest() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[
                folder("/home/a", "/home", 100, 1),
                folder("/home/b", "/home", 900, 1),
                folder("/home/c", "/home", 500, 1),
            ],
            1_000,
        )
        .unwrap();

        let kids = db.folder_children("/home", 2).unwrap();

        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].name, "b");
        assert_eq!(kids[1].name, "c");
    }

    #[test]
    fn has_children_marks_what_can_be_drilled_into() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[
                folder("/home/parent", "/home", 900, 2),
                folder("/home/parent/kid", "/home/parent", 800, 1),
                folder("/home/leaf", "/home", 100, 1),
            ],
            1_000,
        )
        .unwrap();

        let kids = db.folder_children("/home", 5).unwrap();
        let parent = kids.iter().find(|k| k.name == "parent").unwrap();
        let leaf = kids.iter().find(|k| k.name == "leaf").unwrap();

        assert!(parent.has_children);
        assert!(!leaf.has_children);
    }

    #[test]
    fn a_replace_removes_folders_that_no_longer_exist() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[
                folder("/home/keep", "/home", 100, 1),
                folder("/home/deleted", "/home", 900, 1),
            ],
            1_000,
        )
        .unwrap();

        db.replace_folder_tree(
            Path::new("/home"),
            &[folder("/home/keep", "/home", 100, 1)],
            2_000,
        )
        .unwrap();

        let kids = db.folder_children("/home", 10).unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].name, "keep");
    }

    #[test]
    fn a_replace_leaves_other_roots_alone() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[folder("/home/a", "/home", 100, 1)],
            1_000,
        )
        .unwrap();
        db.replace_folder_tree(
            Path::new("/data"),
            &[folder("/data/b", "/data", 100, 1)],
            1_000,
        )
        .unwrap();

        // Rescanning /home must not disturb /data.
        db.replace_folder_tree(
            Path::new("/home"),
            &[folder("/home/a", "/home", 200, 1)],
            2_000,
        )
        .unwrap();

        assert_eq!(db.folder_children("/data", 10).unwrap().len(), 1);
    }

    #[test]
    fn verify_updates_only_the_rows_it_touches() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[
                folder("/home/fresh", "/home", 100, 1),
                folder("/home/stale", "/home", 900, 1),
            ],
            1_000,
        )
        .unwrap();

        db.update_folder_rows(&[folder("/home/fresh", "/home", 150, 2)], 5_000)
            .unwrap();

        let fresh = db.folder_entry("/home/fresh").unwrap().unwrap();
        let stale = db.folder_entry("/home/stale").unwrap().unwrap();

        assert_eq!(fresh.size_bytes, 150);
        assert_eq!(fresh.scanned_at, 5_000);
        assert_eq!(stale.size_bytes, 900);
        assert_eq!(
            stale.scanned_at, 1_000,
            "an untouched row keeps its original timestamp"
        );
    }

    #[test]
    fn verify_does_not_move_the_full_scan_timestamp() {
        let db = mk_db();
        db.replace_folder_tree(
            Path::new("/home"),
            &[folder("/home/a", "/home", 100, 1)],
            1_000,
        )
        .unwrap();

        db.update_folder_rows(&[folder("/home/a", "/home", 200, 1)], 9_000)
            .unwrap();

        assert_eq!(
            db.last_full_scan_at(),
            Some(1_000),
            "only a full scan may advance the full-scan clock"
        );
    }

    #[test]
    fn unreadable_counts_survive_a_round_trip() {
        let db = mk_db();
        let mut f = folder("/home/denied", "/home", 100, 1);
        f.unreadable = 7;
        db.replace_folder_tree(Path::new("/home"), &[f], 1_000)
            .unwrap();

        let row = db.folder_entry("/home/denied").unwrap().unwrap();
        assert_eq!(row.unreadable, 7);
    }

    #[test]
    fn an_unknown_path_is_none_not_an_error() {
        let db = mk_db();
        assert_eq!(db.folder_entry("/nope").unwrap(), None);
        assert!(db.folder_children("/nope", 5).unwrap().is_empty());
    }
}
