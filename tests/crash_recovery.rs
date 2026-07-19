use std::path::PathBuf;

use ork_storage::wal::WalReplay;
use ork_storage::writer_actor::{WriteOp, WriterActor};
use rusqlite::Connection;
use tokio::sync::oneshot;

struct TestDb {
    path: PathBuf,
}

impl TestDb {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.into_path().join("test.db");
        let conn = Connection::open(&path).expect("open");

        WalReplay::ensure_write_journal(&conn).expect("write_journal");

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv_testns (
                key TEXT PRIMARY KEY,
                value BLOB NOT NULL
            )",
        )
        .expect("kv_testns");

        conn.close().ok();
        Self { path }
    }

    fn connect(&self) -> Connection {
        Connection::open(&self.path).expect("reopen")
    }
}

fn insert_journal_entry(
    conn: &Connection,
    op_id: &str,
    namespace: &str,
    key: &str,
    value: &[u8],
    op_type_str: &str,
    applied: bool,
) {
    conn.execute(
        "INSERT INTO write_journal (op_id, namespace, key, value, op_type, applied)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![op_id, namespace, key, value, op_type_str, applied as i32],
    )
    .expect("insert journal");
}

fn count_unapplied(conn: &Connection) -> i32 {
    conn.query_row(
        "SELECT COUNT(*) FROM write_journal WHERE applied = 0",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

fn count_applied(conn: &Connection) -> i32 {
    conn.query_row(
        "SELECT COUNT(*) FROM write_journal WHERE applied = 1",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

#[test]
fn test_wal_replay_on_startup() {
    let db = TestDb::new();

    {
        let conn = db.connect();
        insert_journal_entry(&conn, "op-001", "testns", "k1", b"v1", "SET", false);
        insert_journal_entry(&conn, "op-002", "testns", "k2", b"v2", "SET", false);
        insert_journal_entry(&conn, "op-003", "testns", "k3", b"v3", "SET", false);
    }

    let replay = WalReplay::new(&db.path).expect("WalReplay::new");
    let report = replay.replay_pending().expect("replay");

    assert_eq!(report.total_ops, 3);
    assert_eq!(report.replayed, 3);
    assert_eq!(report.failed, 0);

    let conn = db.connect();
    assert_eq!(count_unapplied(&conn), 0);
    assert_eq!(count_applied(&conn), 3);

    let kv_count: i32 = conn
        .query_row("SELECT COUNT(*) FROM kv_testns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kv_count, 3);
}

#[test]
fn test_idempotent_replay() {
    let db = TestDb::new();

    {
        let conn = db.connect();
        insert_journal_entry(&conn, "op-a", "testns", "key1", b"val1", "SET", false);
        insert_journal_entry(&conn, "op-b", "testns", "key2", b"val2", "SET", false);
    }

    let replay = WalReplay::new(&db.path).expect("WalReplay::new");

    let report1 = replay.replay_pending().expect("first replay");
    assert_eq!(report1.total_ops, 2);
    assert_eq!(report1.replayed, 2);
    assert_eq!(report1.failed, 0);

    let report2 = replay.replay_pending().expect("second replay");
    assert_eq!(report2.total_ops, 0, "second replay: all already applied");
    assert_eq!(report2.replayed, 0);
    assert_eq!(report2.failed, 0);

    let conn = db.connect();
    assert_eq!(count_unapplied(&conn), 0);
    assert_eq!(count_applied(&conn), 2);
}

#[tokio::test]
async fn test_crash_during_write() {
    let db = TestDb::new();

    {
        let conn = db.connect();
        insert_journal_entry(&conn, "crash-001", "testns", "alpha", b"aaa", "SET", false);
        insert_journal_entry(&conn, "crash-002", "testns", "beta", b"bbb", "SET", false);
        insert_journal_entry(&conn, "crash-003", "testns", "gamma", b"ggg", "SET", false);
    }

    let mut actor = WriterActor::new(&db.path);

    let (tx1, rx1) = oneshot::channel();
    actor
        .write(WriteOp::Set {
            namespace: "testns".into(),
            key: "alpha".into(),
            value: b"aaa".to_vec(),
            reply: tx1,
        })
        .await
        .expect("send");
    rx1.await.expect("recv").expect("write");

    let (tx2, rx2) = oneshot::channel();
    actor
        .write(WriteOp::Set {
            namespace: "testns".into(),
            key: "beta".into(),
            value: b"bbb".to_vec(),
            reply: tx2,
        })
        .await
        .expect("send");
    rx2.await.expect("recv").expect("write");

    actor.shutdown().await;

    let replay = WalReplay::new(&db.path).expect("WalReplay::new");
    let report = replay.replay_pending().expect("recovery");

    assert_eq!(report.total_ops, 3);
    assert_eq!(report.replayed, 3);
    assert_eq!(report.failed, 0);

    let conn = db.connect();
    assert_eq!(count_unapplied(&conn), 0);

    let vals: Vec<String> = conn
        .prepare("SELECT value FROM kv_testns ORDER BY key")
        .unwrap()
        .query_map([], |r| {
            let v: Vec<u8> = r.get(0)?;
            Ok(String::from_utf8_lossy(&v).to_string())
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(vals, vec!["aaa", "bbb", "ggg"]);
}
