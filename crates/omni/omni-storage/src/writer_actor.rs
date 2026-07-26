use std::time::Duration;

use rusqlite::types::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Instant};

use crate::traits::StorageError;

pub enum WriteOp {
    Set {
        namespace: String,
        key: String,
        value: Vec<u8>,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Delete {
        namespace: String,
        key: String,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Execute {
        sql: String,
        params: Vec<Value>,
        reply: oneshot::Sender<Result<usize, StorageError>>,
    },
    Batch {
        ops: Vec<WriteOp>,
    },
    Vacuum,
    Flush {
        done: Option<oneshot::Sender<()>>,
    },
    Shutdown,
}

const BATCH_SIZE: usize = 64;
const BATCH_TIMEOUT_MS: u64 = 50;
const WAL_CHECKPOINT_INTERVAL: u32 = 1000;
const MAX_RETRIES: u32 = 5;
const BASE_BACKOFF_MS: u64 = 1;

pub struct WriterActor {
    tx: Option<mpsc::Sender<WriteOp>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl WriterActor {
    pub fn new(db_path: &std::path::Path) -> Self {
        let (tx, mut rx) = mpsc::channel::<WriteOp>(1024);
        let path = db_path.to_path_buf();

        let handle = tokio::spawn(async move {
            let conn = match rusqlite::Connection::open(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(%e, "WriterActor: failed to open SQLite");
                    return;
                }
            };

            if let Err(e) =
                conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            {
                tracing::error!(%e, "WriterActor: failed to set pragmas");
            }

            let mut pending: Vec<WriteOp> = Vec::with_capacity(BATCH_SIZE);
            let mut batch_count: u32 = 0;

            loop {
                let first = rx.recv().await;

                match first {
                    Some(op) => pending.push(op),
                    None => break,
                }

                while let Ok(op) = rx.try_recv() {
                    pending.push(op);
                }

                let deadline = Instant::now() + Duration::from_millis(BATCH_TIMEOUT_MS);
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if pending.len() >= BATCH_SIZE || remaining.is_zero() {
                        break;
                    }
                    match timeout(remaining, rx.recv()).await {
                        Ok(Some(op)) => pending.push(op),
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }

                let has_shutdown =
                    pending.iter().any(|op| matches!(op, WriteOp::Shutdown));

                let mut flushes: Vec<oneshot::Sender<()>> = Vec::new();
                for i in (0..pending.len()).rev() {
                    if matches!(pending[i], WriteOp::Flush { .. }) {
                        let WriteOp::Flush { done } = pending.swap_remove(i) else { unreachable!() };
                        if let Some(d) = done {
                            flushes.push(d);
                        }
                    }
                }

                let batch = std::mem::take(&mut pending);
                Self::flush_batch(&conn, batch);
                batch_count += 1;

                for done in flushes {
                    let _ = done.send(());
                }

                if batch_count % WAL_CHECKPOINT_INTERVAL == 0 {
                    if let Err(e) =
                        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    {
                        tracing::warn!(%e, "WriterActor: WAL checkpoint failed");
                    }
                }

                if has_shutdown {
                    break;
                }
            }

            if !pending.is_empty() {
                Self::flush_batch(&conn, std::mem::take(&mut pending));
            }

            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let _ = conn.execute_batch("PRAGMA optimize;");
        });

        Self {
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    fn retry_execute<F, T>(mut f: F) -> Result<T, StorageError>
    where
        F: FnMut() -> Result<T, rusqlite::Error>,
    {
        let mut retries: u32 = 0;
        loop {
            match f() {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let is_busy = matches!(
                        e,
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error {
                                code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                                ..
                            },
                            _,
                        )
                    );
                    if !is_busy || retries >= MAX_RETRIES {
                        return Err(StorageError::Internal(e.to_string()));
                    }
                    let backoff =
                        Duration::from_millis(BASE_BACKOFF_MS * 2u64.pow(retries));
                    std::thread::sleep(backoff);
                    retries += 1;
                    tracing::warn!(retries, "WriterActor: SQLITE_BUSY retry");
                }
            }
        }
    }

    fn flatten_batch(ops: Vec<WriteOp>) -> Vec<WriteOp> {
        let mut result: Vec<WriteOp> = Vec::with_capacity(ops.len());
        for op in ops {
            match op {
                WriteOp::Batch { ops: inner } => {
                    result.extend(Self::flatten_batch(inner));
                }
                other => result.push(other),
            }
        }
        result
    }

    fn flush_batch(conn: &rusqlite::Connection, raw_batch: Vec<WriteOp>) {
        let batch = Self::flatten_batch(raw_batch);
        let use_txn = batch.len() > 1;

        if use_txn {
            if let Err(e) = Self::retry_execute(|| conn.execute_batch("BEGIN IMMEDIATE"))
            {
                for op in batch {
                    Self::send_error(op, StorageError::Internal(e.to_string()));
                }
                return;
            }
        }

        for op in batch {
            match op {
                WriteOp::Set {
                    namespace,
                    key,
                    value,
                    reply,
                } => {
                    let table = format!("kv_{namespace}");
                    let sql = format!(
                        "INSERT INTO \"{table}\" (key, value) VALUES (?1, ?2) \
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value"
                    );
                    let result =
                        Self::retry_execute(|| {
                            conn.execute(&sql, rusqlite::params![key, value])
                        })
                        .map(|_| ());
                    let _ = reply.send(result);
                }
                WriteOp::Delete {
                    namespace,
                    key,
                    reply,
                } => {
                    let table = format!("kv_{namespace}");
                    let sql = format!("DELETE FROM \"{table}\" WHERE key = ?1");
                    let result = Self::retry_execute(|| {
                        conn.execute(&sql, rusqlite::params![key])
                    })
                    .map(|_| ());
                    let _ = reply.send(result);
                }
                WriteOp::Execute {
                    sql,
                    params,
                    reply,
                } => {
                    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                        params
                            .iter()
                            .map(|v| v as &dyn rusqlite::types::ToSql)
                            .collect();
                    let result = Self::retry_execute(|| {
                        conn.execute(
                            &sql,
                            rusqlite::params_from_iter(param_refs.iter().copied()),
                        )
                    });
                    let _ = reply.send(result);
                }
                WriteOp::Batch { ops } => {
                    // Already flattened, but handle recursively as safety net
                    Self::flush_batch(conn, ops);
                }
                WriteOp::Vacuum => {
                    let _ = Self::retry_execute(|| conn.execute_batch("VACUUM"));
                }
                WriteOp::Flush { .. } | WriteOp::Shutdown => {}
            }
        }

        if use_txn {
            if let Err(e) = Self::retry_execute(|| conn.execute_batch("COMMIT")) {
                tracing::error!(%e, "WriterActor: COMMIT failed after retries");
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }

    fn send_error(op: WriteOp, err: StorageError) {
        match op {
            WriteOp::Set { reply, .. } | WriteOp::Delete { reply, .. } => {
                let _ = reply.send(Err(err));
            }
            WriteOp::Execute { reply, .. } => {
                let _ = reply.send(Err(err));
            }
            WriteOp::Batch { ops } => {
                for inner in ops {
                    Self::send_error(inner, err.clone());
                }
            }
            WriteOp::Vacuum | WriteOp::Shutdown | WriteOp::Flush { .. } => {}
        }
    }

    pub async fn write(&self, op: WriteOp) -> Result<(), StorageError> {
        self.tx
            .as_ref()
            .ok_or_else(|| {
                StorageError::Internal("WriterActor channel closed".into())
            })?
            .send(op)
            .await
            .map_err(|_| StorageError::Internal("WriterActor channel closed".into()))
    }

    pub async fn flush(&self) {
        if let Some(tx) = &self.tx {
            let (done, rx) = oneshot::channel();
            let _ = tx.send(WriteOp::Flush { done: Some(done) }).await;
            let _ = rx.await;
        }
    }

    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(WriteOp::Shutdown).await;
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}
