use std::path::Path;

use serde::{Deserialize, Serialize};
use xai_sqlite_journal::JournalMode;

use crate::traits::StorageError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingOp {
    pub op_id: String,
    pub namespace: String,
    pub key: String,
    pub value: Option<Vec<u8>>,
    pub op_type: OpType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpType {
    Set,
    Delete,
}

#[derive(Debug, Default)]
pub struct ReplayReport {
    pub total_ops: u64,
    pub replayed: u64,
    pub skipped: u64,
    pub failed: u64,
}

pub struct WalReplay {
    journal_path: std::path::PathBuf,
    journal_mode: JournalMode,
}

impl WalReplay {
    pub fn new(journal_path: &Path) -> Result<Self, StorageError> {
        let journal_mode = JournalMode::for_db_path(journal_path);
        Ok(Self { journal_path: journal_path.to_path_buf(), journal_mode })
    }

    pub fn replay_pending(&self) -> Result<ReplayReport, StorageError> {
        let mut report = ReplayReport::default();

        let conn = self
            .journal_mode
            .open(&self.journal_path)
            .map_err(|e| StorageError::Internal(format!("Journal open: {e}")))?;

        let has_table: bool = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='write_journal'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(false);

        if !has_table {
            return Ok(report);
        }

        let mut stmt = conn
            .prepare("SELECT op_id, namespace, key, value, op_type FROM write_journal WHERE applied = 0 ORDER BY id ASC")
            .map_err(|e| StorageError::Internal(e.to_string()))?;

        let ops: Vec<PendingOp> = stmt
            .query_map([], |r| {
                let value: Option<Vec<u8>> = r.get::<_, Option<Vec<u8>>>(3)?;
                let op_type_str: String = r.get(4)?;
                Ok(PendingOp {
                    op_id: r.get(0)?,
                    namespace: r.get(1)?,
                    key: r.get(2)?,
                    value,
                    op_type: match op_type_str.as_str() {
                        "SET" => OpType::Set,
                        _ => OpType::Delete,
                    },
                })
            })
            .map_err(|e| StorageError::Internal(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        report.total_ops = ops.len() as u64;

        for op in &ops {
            if self.is_op_applied(&conn, &op.op_id) {
                report.skipped += 1;
                continue;
            }

            match self.apply_op(&conn, op) {
                Ok(_) => {
                    if Self::mark_applied(&conn, &op.op_id).is_ok() {
                        report.replayed += 1;
                    } else {
                        report.failed += 1;
                        tracing::error!(
                            op_id = %op.op_id,
                            "Replay succeeded but failed to mark applied; op left as applied=0 for retry"
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(op_id = %op.op_id, error = %e, "Replay failed for op");
                    report.failed += 1;
                }
            }
        }

        Ok(report)
    }

    fn is_op_applied(&self, conn: &rusqlite::Connection, op_id: &str) -> bool {
        conn.query_row(
            "SELECT applied FROM write_journal WHERE op_id = ?1",
            rusqlite::params![op_id],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    fn apply_op(&self, conn: &rusqlite::Connection, op: &PendingOp) -> Result<(), StorageError> {
        let table = format!("kv_{}", op.namespace);
        match op.op_type {
            OpType::Set => {
                let sql = format!(
                    "INSERT INTO {table} (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value"
                );
                conn.execute(&sql, rusqlite::params![op.key, op.value])
                    .map_err(|e| StorageError::Internal(e.to_string()))?;
            }
            OpType::Delete => {
                let sql = format!("DELETE FROM {table} WHERE key = ?1");
                conn.execute(&sql, rusqlite::params![op.key])
                    .map_err(|e| StorageError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }

    fn mark_applied(conn: &rusqlite::Connection, op_id: &str) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE write_journal SET applied = 1 WHERE op_id = ?1",
            rusqlite::params![op_id],
        )
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(())
    }

    pub fn ensure_write_journal(conn: &rusqlite::Connection) -> Result<(), StorageError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS write_journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                op_id TEXT NOT NULL UNIQUE,
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value BLOB,
                op_type TEXT NOT NULL DEFAULT 'SET',
                applied INTEGER NOT NULL DEFAULT 0,
                ts TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_journal(dir: &TempDir) -> rusqlite::Connection {
        let path = dir.path().join("test_journal.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        WalReplay::ensure_write_journal(&conn).unwrap();
        conn
    }

    fn insert_op(conn: &rusqlite::Connection, op_id: &str, namespace: &str, key: &str, value: &[u8], op_type: &str) {
        conn.execute(
            "INSERT INTO write_journal (op_id, namespace, key, value, op_type, applied) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            rusqlite::params![op_id, namespace, key, value, op_type],
        )
        .unwrap();
    }

    fn insert_op_applied(conn: &rusqlite::Connection, op_id: &str, namespace: &str, key: &str, value: &[u8], op_type: &str) {
        conn.execute(
            "INSERT INTO write_journal (op_id, namespace, key, value, op_type, applied) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            rusqlite::params![op_id, namespace, key, value, op_type],
        )
        .unwrap();
    }

    fn ensure_kv_table(conn: &rusqlite::Connection, namespace: &str) {
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS kv_{namespace} (key TEXT PRIMARY KEY, value BLOB)"
        ))
        .unwrap();
    }

    #[test]
    fn test_idempotent_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_journal.sqlite");

        {
            let conn = open_journal(&dir);
            ensure_kv_table(&conn, "ns1");
            insert_op(&conn, "op-1", "ns1", "k1", b"v1", "SET");
            insert_op(&conn, "op-2", "ns1", "k2", b"v2", "SET");
        }

        let wal = WalReplay::new(&path).unwrap();

        // First replay: both ops should be replayed
        let report1 = wal.replay_pending().unwrap();
        assert_eq!(report1.total_ops, 2);
        assert_eq!(report1.replayed, 2);
        assert_eq!(report1.skipped, 0);
        assert_eq!(report1.failed, 0);

        // Second replay: same WAL file, all ops already applied=1 → nothing to do
        let report2 = wal.replay_pending().unwrap();
        assert_eq!(report2.total_ops, 0, "second replay should find zero pending ops");
        assert_eq!(report2.replayed, 0);
        assert_eq!(report2.skipped, 0);
        assert_eq!(report2.failed, 0);

        // Add more ops, then second replay with both old (applied) and new (pending)
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            insert_op(&conn, "op-3", "ns1", "k3", b"v3", "SET");
            insert_op(&conn, "op-4", "ns1", "k4", b"v4", "SET");
        }

        let report3 = wal.replay_pending().unwrap();
        // Only op-3, op-4 are pending (applied=0); op-1, op-2 already applied=1
        assert_eq!(report3.total_ops, 2);
        assert_eq!(report3.replayed, 2);
        assert_eq!(report3.skipped, 0);
        assert_eq!(report3.failed, 0);

        // Verify data is correct (all 4 keys exist)
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for (k, v) in [("k1", b"v1" as &[u8]), ("k2", b"v2"), ("k3", b"v3"), ("k4", b"v4")] {
                let got: Vec<u8> = conn
                    .query_row("SELECT value FROM kv_ns1 WHERE key = ?1", [k], |r| r.get(0))
                    .unwrap();
                assert_eq!(got, v, "{k} should equal {v:?}");
            }
        }
    }

    #[test]
    fn test_partial_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_journal.sqlite");

        {
            let conn = open_journal(&dir);
            ensure_kv_table(&conn, "ns_ok");
            // op-good: valid, with kv_ns_ok table
            insert_op(&conn, "op-good", "ns_ok", "gk1", b"good_value", "SET");
            // op-bad: references non-existent table kv_ns_missing
            insert_op(&conn, "op-bad", "ns_missing", "bk1", b"bad_value", "SET");
            // op-also-good: valid
            insert_op(&conn, "op-also-good", "ns_ok", "gk2", b"also_good", "SET");
        }

        let wal = WalReplay::new(&path).unwrap();
        let report = wal.replay_pending().unwrap();

        assert_eq!(report.total_ops, 3);
        assert_eq!(report.replayed, 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 1);

        // Verify good ops were applied
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let applied: i32 = conn
                .query_row("SELECT applied FROM write_journal WHERE op_id = 'op-good'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(applied, 1);

            let applied_bad: i32 = conn
                .query_row("SELECT applied FROM write_journal WHERE op_id = 'op-bad'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(applied_bad, 0, "failed op should remain applied=0");

            let applied_also: i32 = conn
                .query_row("SELECT applied FROM write_journal WHERE op_id = 'op-also-good'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(applied_also, 1);
        }
    }

    #[test]
    fn test_recovery_after_crash() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_journal.sqlite");

        // Simulate pre-crash state: some ops applied, some not
        {
            let conn = open_journal(&dir);
            ensure_kv_table(&conn, "ns_recv");
            // recv-1 was already applied before crash: journal entry + actual data
            insert_op_applied(&conn, "recv-1", "ns_recv", "rk1", b"pre-crash", "SET");
            conn.execute("INSERT INTO kv_ns_recv (key, value) VALUES ('rk1', x'7072652d6372617368')", [])
                .unwrap();
            // recv-2, recv-3: written to journal but not yet applied (crash happened)
            insert_op(&conn, "recv-2", "ns_recv", "rk2", b"mid-crash", "SET");
            insert_op(&conn, "recv-3", "ns_recv", "rk3", b"post-crash", "SET");
        }

        // Crash simulation: connection closed, data in journal
        // Recovery: open WAL replay
        let wal = WalReplay::new(&path).unwrap();
        let report = wal.replay_pending().unwrap();

        // recv-1 already applied, should not appear (applied=1 filter)
        // recv-2 and recv-3 should be replayed
        assert_eq!(report.total_ops, 2);
        assert_eq!(report.replayed, 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);

        // Verify all data is present
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let v1: Vec<u8> = conn
                .query_row("SELECT value FROM kv_ns_recv WHERE key = 'rk1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v1, b"pre-crash");
            let v2: Vec<u8> = conn
                .query_row("SELECT value FROM kv_ns_recv WHERE key = 'rk2'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v2, b"mid-crash");
            let v3: Vec<u8> = conn
                .query_row("SELECT value FROM kv_ns_recv WHERE key = 'rk3'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v3, b"post-crash");

            // All should be marked applied
            let count: i32 = conn
                .query_row("SELECT COUNT(*) FROM write_journal WHERE applied = 0", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn test_empty_journal_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_journal.sqlite");

        // No write_journal table at all
        let wal = WalReplay::new(&path).unwrap();
        let report = wal.replay_pending().unwrap();
        assert_eq!(report.total_ops, 0);
        assert_eq!(report.replayed, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn test_delete_op_replay() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_journal.sqlite");

        {
            let conn = open_journal(&dir);
            ensure_kv_table(&conn, "ns_del");
            // Pre-populate some data
            conn.execute("INSERT INTO kv_ns_del (key, value) VALUES ('dk1', x'6461746131')", [])
                .unwrap();
            insert_op(&conn, "del-1", "ns_del", "dk1", b"", "DELETE");
        }

        let wal = WalReplay::new(&path).unwrap();
        let report = wal.replay_pending().unwrap();
        assert_eq!(report.total_ops, 1);
        assert_eq!(report.replayed, 1);
        assert_eq!(report.failed, 0);

        // Verify delete was applied
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let exists: bool = conn
                .query_row("SELECT COUNT(*) FROM kv_ns_del WHERE key = 'dk1'", [], |r| {
                    r.get::<_, i32>(0).map(|c| c > 0)
                })
                .unwrap();
            assert!(!exists, "key should be deleted");
        }
    }

    #[test]
    fn test_mark_applied_failure_handled() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_journal.sqlite");

        {
            let conn = open_journal(&dir);
            ensure_kv_table(&conn, "ns_mark");
            insert_op(&conn, "mark-1", "ns_mark", "mk1", b"mark_val", "SET");
        }

        let wal = WalReplay::new(&path).unwrap();

        // Add a trigger that raises an error on UPDATE to write_journal,
        // causing mark_applied to fail while apply_op succeeds.
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TRIGGER block_mark BEFORE UPDATE ON write_journal
                 BEGIN
                     SELECT RAISE(ABORT, 'mark_applied blocked');
                 END;",
            )
            .unwrap();
        }

        let report = wal.replay_pending().unwrap();
        // apply_op succeeded (data written to kv_ns_mark), mark_applied failed
        assert_eq!(report.total_ops, 1);
        assert_eq!(report.replayed, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 1);

        // Verify data was persisted despite mark failure
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let v: Vec<u8> = conn
                .query_row("SELECT value FROM kv_ns_mark WHERE key = 'mk1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, b"mark_val");
            // Op remains pending (applied=0), ready for retry
            let applied: i32 = conn
                .query_row("SELECT applied FROM write_journal WHERE op_id = 'mark-1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(applied, 0);
        }
    }
}
