//! Olay-log ve diff akisinin DB yazicisi (MASTER-PLAN 8.1/I7, 9.3, 14.3, 17.1).
//!
//! Yazilan tablolar (migrations'tan birebir):
//!   * `agent_events(agent_id, seq, kind, payload_json, ts)`  — 0003
//!   * `messages(agent_id, role, provider_model, content_ref, tokens_in, tokens_out, cost, ts)` — 0003
//!   * `tool_calls(agent_id, tool, args_json, result_ref, status, capability_ok, ts)` — 0004
//!   * `file_touches(agent_id, path, outside_workspace, added, removed, pre_ref, post_ref, ts)` — 0008
//!
//! Dayaniklilik sozlesmesi (8.1):
//!   1. Yan etkili her satir yaziminin **oncesinde** `write_journal`'a idempotent
//!      niyet kaydi dusulur (`op_id` UNIQUE, `applied=0`).
//!   2. Asil INSERT ile `applied=1` guncellemesi **tek** `WriteOp::Batch` icinde
//!      gider; `WriterActor` uzunlugu 1'den buyuk batch'i `BEGIN IMMEDIATE`/
//!      `COMMIT` ile sarar, yani ya ikisi de olur ya hicbiri.
//!   3. Kapanis O(1)'dir: burada flush/bekleme yoktur. `EventWriter` dusurulunce
//!      hicbir sey beklemez; yarim kalan niyetler acilista `recover()` ile
//!      tekrar oynatilir.
//!
//! CAS kullanimi (14.3): `content_ref` / `result_ref` / `pre_ref` / `post_ref`
//! her zaman icerik-hash'idir. `payload_json` / `args_json` kucukse satir icinde
//! kalir, esigi asarsa CAS'a tasinir ve tabloda `{"cas":"<hash>"}` durur.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;
use xai_sqlite_journal::JournalMode;

use crate::cas::CasBlobStore;
use crate::traits::{BlobStore, StorageError};
use crate::wal::WalReplay;
use crate::writer_actor::{WriteOp, WriterActor};

/// Satir ici JSON esigi; bunun uzeri CAS'a tasinir (14.3).
pub const INLINE_JSON_LIMIT: usize = 8 * 1024;

/// Niyet kayitlarinin `write_journal.namespace` degeri. Replay hedefi bu addan
/// turetilen `kv_omni_events` tablosudur (`WalReplay::apply_op` `kv_{namespace}`
/// yazar).
const JOURNAL_NAMESPACE: &str = "omni_events";

/// `write_journal.op_kind` sutunu 0007 semasinda `NOT NULL` ve varsayilansiz;
/// o sema yuruyorsa bu deger yazilir.
const JOURNAL_OP_KIND: &str = "omni_event";

// ---------------------------------------------------------------------------
// Girdi kayitlari
// ---------------------------------------------------------------------------

/// `agent_events` satiri. `seq` yazici tarafindan uretilir.
#[derive(Debug, Clone)]
pub struct AgentEventRecord {
    pub agent_id: i64,
    pub kind: String,
    /// Serilestirilmis olay govdesi; bos birakilabilir.
    pub payload_json: Option<String>,
}

/// `messages` satiri. `content` daima CAS'a gider, tabloda `content_ref` durur.
#[derive(Debug, Clone)]
pub struct MessageRecord {
    pub agent_id: i64,
    pub role: String,
    /// Saglayici/model kimligi; cagiran katmandan gelir, burada sabit yoktur.
    pub provider_model: Option<String>,
    pub content: Vec<u8>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub cost: Option<f64>,
}

/// `tool_calls` satiri. `result` daima CAS'a gider (`result_ref`).
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub agent_id: i64,
    pub tool: String,
    pub args_json: Option<String>,
    pub result: Option<Vec<u8>>,
    pub status: String,
    pub capability_ok: Option<bool>,
}

/// `file_touches` satiri (9.3). `pre`/`post` icerikleri CAS'a gider.
#[derive(Debug, Clone)]
pub struct FileTouchRecord {
    pub agent_id: i64,
    pub path: String,
    /// Calisma dizini disi mi — kanca FS trait duzeyinde oldugu icin bu bilgi
    /// yazimi engellemez, yalniz isaretler (9.3).
    pub outside_workspace: bool,
    pub added: i64,
    pub removed: i64,
    pub pre: Option<Vec<u8>>,
    pub post: Option<Vec<u8>>,
}

/// Tek bir yazimin sonucu.
#[derive(Debug, Clone)]
pub struct WriteReceipt {
    pub op_id: String,
    pub ts: String,
    /// Yalniz `agent_events` icin dolu.
    pub seq: Option<i64>,
    /// Bu yazim sirasinda CAS'a konan icerik hash'leri.
    pub refs: Vec<String>,
}

/// `recover()` ozeti.
#[derive(Debug, Default, Clone)]
pub struct RecoveryReport {
    /// `WalReplay` tarafindan sahne tablosuna tasinan niyet sayisi.
    pub replayed: u64,
    /// Sahne tablosundan asil tabloya geri uygulanan niyet sayisi.
    pub reapplied: u64,
    /// Cozulemeyen/uygulanamayan niyet sayisi.
    pub failed: u64,
}

// ---------------------------------------------------------------------------
// Niyet govdesi
// ---------------------------------------------------------------------------

/// Niyet kaydinin cozulmus hali: CAS ref'leri ve `seq` bu noktada bellidir,
/// yani replay sirasinda hicbir yan etki tekrar uretilmez.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ResolvedWrite {
    AgentEvent {
        agent_id: i64,
        seq: i64,
        kind: String,
        payload_json: Option<String>,
    },
    Message {
        agent_id: i64,
        role: String,
        provider_model: Option<String>,
        content_ref: Option<String>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
        cost: Option<f64>,
    },
    ToolCall {
        agent_id: i64,
        tool: String,
        args_json: Option<String>,
        result_ref: Option<String>,
        status: String,
        capability_ok: Option<bool>,
    },
    FileTouch {
        agent_id: i64,
        path: String,
        outside_workspace: bool,
        added: i64,
        removed: i64,
        pre_ref: Option<String>,
        post_ref: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalIntent {
    op_id: String,
    ts: String,
    write: ResolvedWrite,
}

fn opt_text(v: Option<String>) -> Value {
    v.map_or(Value::Null, Value::Text)
}

fn opt_int(v: Option<i64>) -> Value {
    v.map_or(Value::Null, Value::Integer)
}

fn opt_real(v: Option<f64>) -> Value {
    v.map_or(Value::Null, Value::Real)
}

impl ResolvedWrite {
    fn seq(&self) -> Option<i64> {
        match self {
            ResolvedWrite::AgentEvent { seq, .. } => Some(*seq),
            _ => None,
        }
    }

    /// Idempotent INSERT: `WHERE NOT EXISTS` korumasi sayesinde ayni niyet
    /// tekrar oynatilsa da ikinci satir olusmaz (8.1 adim 4).
    fn insert_sql(&self, ts: &str) -> (String, Vec<Value>) {
        match self {
            ResolvedWrite::AgentEvent {
                agent_id,
                seq,
                kind,
                payload_json,
            } => (
                "INSERT INTO agent_events (agent_id, seq, kind, payload_json, ts) \
                 SELECT ?1, ?2, ?3, ?4, ?5 \
                 WHERE NOT EXISTS (SELECT 1 FROM agent_events WHERE agent_id = ?1 AND seq = ?2)"
                    .to_string(),
                vec![
                    Value::Integer(*agent_id),
                    Value::Integer(*seq),
                    Value::Text(kind.clone()),
                    opt_text(payload_json.clone()),
                    Value::Text(ts.to_string()),
                ],
            ),
            ResolvedWrite::Message {
                agent_id,
                role,
                provider_model,
                content_ref,
                tokens_in,
                tokens_out,
                cost,
            } => (
                "INSERT INTO messages \
                 (agent_id, role, provider_model, content_ref, tokens_in, tokens_out, cost, ts) \
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8 \
                 WHERE NOT EXISTS \
                 (SELECT 1 FROM messages WHERE agent_id = ?1 AND role = ?2 AND ts = ?8)"
                    .to_string(),
                vec![
                    Value::Integer(*agent_id),
                    Value::Text(role.clone()),
                    opt_text(provider_model.clone()),
                    opt_text(content_ref.clone()),
                    opt_int(*tokens_in),
                    opt_int(*tokens_out),
                    opt_real(*cost),
                    Value::Text(ts.to_string()),
                ],
            ),
            ResolvedWrite::ToolCall {
                agent_id,
                tool,
                args_json,
                result_ref,
                status,
                capability_ok,
            } => (
                "INSERT INTO tool_calls \
                 (agent_id, tool, args_json, result_ref, status, capability_ok, ts) \
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7 \
                 WHERE NOT EXISTS \
                 (SELECT 1 FROM tool_calls WHERE agent_id = ?1 AND tool = ?2 AND ts = ?7)"
                    .to_string(),
                vec![
                    Value::Integer(*agent_id),
                    Value::Text(tool.clone()),
                    opt_text(args_json.clone()),
                    opt_text(result_ref.clone()),
                    Value::Text(status.clone()),
                    opt_int(capability_ok.map(i64::from)),
                    Value::Text(ts.to_string()),
                ],
            ),
            ResolvedWrite::FileTouch {
                agent_id,
                path,
                outside_workspace,
                added,
                removed,
                pre_ref,
                post_ref,
            } => (
                "INSERT INTO file_touches \
                 (agent_id, path, outside_workspace, added, removed, pre_ref, post_ref, ts) \
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8 \
                 WHERE NOT EXISTS \
                 (SELECT 1 FROM file_touches WHERE agent_id = ?1 AND path = ?2 AND ts = ?8)"
                    .to_string(),
                vec![
                    Value::Integer(*agent_id),
                    Value::Text(path.clone()),
                    Value::Integer(i64::from(*outside_workspace)),
                    Value::Integer(*added),
                    Value::Integer(*removed),
                    opt_text(pre_ref.clone()),
                    opt_text(post_ref.clone()),
                    Value::Text(ts.to_string()),
                ],
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// write_journal sema uyumu
// ---------------------------------------------------------------------------

/// `write_journal` deponun iki ayri semasiyla var olabilir (0007 vs `wal.rs`).
/// Hangi sutunlarin bulundugunu acilista tespit edip INSERT'i ona gore kurariz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalCol {
    OpId,
    Namespace,
    Key,
    Value,
    OpType,
    OpKind,
    PayloadRef,
}

#[derive(Debug, Clone)]
struct JournalLayout {
    cols: Vec<JournalCol>,
    sql: String,
}

impl JournalLayout {
    fn detect(conn: &rusqlite::Connection) -> Result<Self, StorageError> {
        let present = table_columns(conn, "write_journal")?;

        let mut cols = vec![JournalCol::OpId];
        for (name, col) in [
            ("namespace", JournalCol::Namespace),
            ("key", JournalCol::Key),
            ("value", JournalCol::Value),
            ("op_type", JournalCol::OpType),
            ("op_kind", JournalCol::OpKind),
            ("payload_ref", JournalCol::PayloadRef),
        ] {
            if present.iter().any(|c| c == name) {
                cols.push(col);
            }
        }

        let names: Vec<&str> = cols.iter().map(|c| c.name()).collect();
        let placeholders: Vec<String> =
            (1..=cols.len()).map(|i| format!("?{i}")).collect();

        // `applied` her iki semada da mevcut; niyet kaydi daima 0 ile acilir.
        let sql = format!(
            "INSERT OR IGNORE INTO write_journal ({}, applied) VALUES ({}, 0)",
            names.join(", "),
            placeholders.join(", ")
        );

        Ok(Self { cols, sql })
    }

    fn params(&self, op_id: &str, payload: &[u8]) -> Vec<Value> {
        self.cols
            .iter()
            .map(|c| match c {
                JournalCol::OpId => Value::Text(op_id.to_string()),
                JournalCol::Namespace => Value::Text(JOURNAL_NAMESPACE.to_string()),
                JournalCol::Key => Value::Text(op_id.to_string()),
                JournalCol::Value => Value::Blob(payload.to_vec()),
                JournalCol::OpType => Value::Text("SET".to_string()),
                JournalCol::OpKind => Value::Text(JOURNAL_OP_KIND.to_string()),
                JournalCol::PayloadRef => Value::Null,
            })
            .collect()
    }
}

impl JournalCol {
    fn name(self) -> &'static str {
        match self {
            JournalCol::OpId => "op_id",
            JournalCol::Namespace => "namespace",
            JournalCol::Key => "key",
            JournalCol::Value => "value",
            JournalCol::OpType => "op_type",
            JournalCol::OpKind => "op_kind",
            JournalCol::PayloadRef => "payload_ref",
        }
    }
}

fn table_columns(
    conn: &rusqlite::Connection,
    table: &str,
) -> Result<Vec<String>, StorageError> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info(?1)")
        .map_err(|e| StorageError::Internal(format!("pragma_table_info hazirlanamadi: {e}")))?;
    let rows = stmt
        .query_map([table], |r| r.get::<_, String>(0))
        .map_err(|e| StorageError::Internal(format!("pragma_table_info sorgusu: {e}")))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| StorageError::Internal(format!("sutun okunamadi: {e}")))?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// EventWriter
// ---------------------------------------------------------------------------

/// Olay-log ve diff akisinin tek yazicisi.
///
/// Tum satir yazimlari [`WriterActor`] uzerinden sirali gider; bu tip yaris
/// uretmez. Kapanis O(1)'dir — `Drop` hicbir sey beklemez, flush yapmaz (8.1).
pub struct EventWriter {
    db_path: PathBuf,
    writer: Arc<WriterActor>,
    cas: CasBlobStore,
    /// Okuma/DDL baglantisi; yazimlar buradan gecmez.
    read_conn: Mutex<rusqlite::Connection>,
    journal: JournalLayout,
    /// agent_id -> sonraki `seq`. Tabloda UNIQUE kisit yok, sirayi uygulama
    /// garanti eder.
    next_seq: DashMap<i64, i64>,
    inline_limit: usize,
}

impl EventWriter {
    /// Verilmis bir [`WriterActor`] uzerine kurar.
    ///
    /// `db_path` [`JournalMode::effective_db_path`] ile normalize edilir; ayni
    /// yolu `WriterActor`'a da vermek cagiranin sorumlulugundadir (bkz.
    /// [`EventWriter::effective_db_path`]).
    pub fn with_writer(
        db_path: &Path,
        cas: CasBlobStore,
        writer: Arc<WriterActor>,
    ) -> Result<Self, StorageError> {
        let path = Self::effective_db_path(db_path);
        let conn = open_db(db_path)?;

        // Niyet tablosu + replay hedefi hazir olmali.
        WalReplay::ensure_write_journal(&conn)?;
        align_journal_schema(&conn)?;
        ensure_replay_staging(&conn)?;

        let journal = JournalLayout::detect(&conn)?;

        Ok(Self {
            db_path: path,
            writer,
            cas,
            read_conn: Mutex::new(conn),
            journal,
            next_seq: DashMap::new(),
            inline_limit: INLINE_JSON_LIMIT,
        })
    }

    /// Kendi [`WriterActor`]'unu kurar. Tokio runtime icinde cagrilmalidir
    /// (`WriterActor::new` gorev spawn eder).
    pub fn open(db_path: &Path, cas_base: &Path) -> Result<Self, StorageError> {
        let path = Self::effective_db_path(db_path);
        let cas = CasBlobStore::new(cas_base)?;
        let writer = Arc::new(WriterActor::new(&path));
        Self::with_writer(&path, cas, writer)
    }

    /// `xai-sqlite-journal`'in secmis oldugu gercek dosya yolu. Ag dosya
    /// sisteminde `foo.db` -> `foo.h-<host>.db` olabilir; islem idempotenttir.
    pub fn effective_db_path(db_path: &Path) -> PathBuf {
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        JournalMode::for_db_path(db_path).effective_db_path(db_path)
    }

    /// Satir ici JSON esigini degistirir (14.3).
    pub fn with_inline_limit(mut self, limit: usize) -> Self {
        self.inline_limit = limit;
        self
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn cas(&self) -> &CasBlobStore {
        &self.cas
    }

    // -- yazim yollari ------------------------------------------------------

    /// `agent_events`'e olay dusurur; uretilen `seq` makbuzda doner (17.1).
    pub async fn record_event(
        &self,
        rec: AgentEventRecord,
    ) -> Result<WriteReceipt, StorageError> {
        let mut refs = Vec::new();
        let payload_json = match rec.payload_json {
            Some(p) => Some(self.maybe_offload(p, &mut refs).await?),
            None => None,
        };
        let seq = self.allocate_seq(rec.agent_id)?;
        let write = ResolvedWrite::AgentEvent {
            agent_id: rec.agent_id,
            seq,
            kind: rec.kind,
            payload_json,
        };
        self.journal_and_apply(write, refs).await
    }

    /// `messages`'a satir dusurur; `content` CAS'a gider (14.3).
    pub async fn record_message(
        &self,
        rec: MessageRecord,
    ) -> Result<WriteReceipt, StorageError> {
        let mut refs = Vec::new();
        let content_ref = self.store_blob(Some(rec.content), &mut refs).await?;
        let write = ResolvedWrite::Message {
            agent_id: rec.agent_id,
            role: rec.role,
            provider_model: rec.provider_model,
            content_ref,
            tokens_in: rec.tokens_in,
            tokens_out: rec.tokens_out,
            cost: rec.cost,
        };
        self.journal_and_apply(write, refs).await
    }

    /// `tool_calls`'a denetim satiri dusurur (9.1); `result` CAS'a gider.
    pub async fn record_tool_call(
        &self,
        rec: ToolCallRecord,
    ) -> Result<WriteReceipt, StorageError> {
        let mut refs = Vec::new();
        let args_json = match rec.args_json {
            Some(a) => Some(self.maybe_offload(a, &mut refs).await?),
            None => None,
        };
        let result_ref = self.store_blob(rec.result, &mut refs).await?;
        let write = ResolvedWrite::ToolCall {
            agent_id: rec.agent_id,
            tool: rec.tool,
            args_json,
            result_ref,
            status: rec.status,
            capability_ok: rec.capability_ok,
        };
        self.journal_and_apply(write, refs).await
    }

    /// `file_touches`'a diff satiri dusurur (9.3); `pre`/`post` CAS'a gider.
    pub async fn record_file_touch(
        &self,
        rec: FileTouchRecord,
    ) -> Result<WriteReceipt, StorageError> {
        let mut refs = Vec::new();
        let pre_ref = self.store_blob(rec.pre, &mut refs).await?;
        let post_ref = self.store_blob(rec.post, &mut refs).await?;
        let write = ResolvedWrite::FileTouch {
            agent_id: rec.agent_id,
            path: rec.path,
            outside_workspace: rec.outside_workspace,
            added: rec.added,
            removed: rec.removed,
            pre_ref,
            post_ref,
        };
        self.journal_and_apply(write, refs).await
    }

    // -- kurtarma -----------------------------------------------------------

    /// Acilis kurtarmasi (8.1 adim 4): once `WalReplay` `applied=0` niyetleri
    /// sahne tablosuna tasir, sonra sahne tablosundaki niyetler asil tablolara
    /// idempotent bicimde geri uygulanir. Yarida kesilirse sahne satiri yerinde
    /// kalir, bir sonraki cagri devam eder.
    pub fn recover(&self) -> Result<RecoveryReport, StorageError> {
        let mut report = RecoveryReport::default();

        let replay = WalReplay::new(&self.db_path)?.replay_pending()?;
        report.replayed = replay.replayed;
        report.failed += replay.failed;

        let conn = self.read_conn.lock();
        ensure_replay_staging(&conn)?;

        let staged: Vec<(String, Vec<u8>)> = {
            let mut stmt = conn
                .prepare(&format!("SELECT key, value FROM kv_{JOURNAL_NAMESPACE}"))
                .map_err(|e| StorageError::Internal(format!("sahne sorgusu: {e}")))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
                })
                .map_err(|e| StorageError::Internal(format!("sahne okuma: {e}")))?;
            let mut out = Vec::new();
            for row in rows {
                match row {
                    Ok(v) => out.push(v),
                    Err(e) => {
                        tracing::warn!(%e, "sahne satiri okunamadi, atlaniyor");
                        report.failed += 1;
                    }
                }
            }
            out
        };

        for (key, payload) in staged {
            let intent: JournalIntent = match serde_json::from_slice(&payload) {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!(op_id = %key, %e, "niyet govdesi cozulemedi");
                    report.failed += 1;
                    continue;
                }
            };

            let (sql, params) = intent.write.insert_sql(&intent.ts);
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();

            let applied = conn.execute(
                &sql,
                rusqlite::params_from_iter(param_refs.iter().copied()),
            );
            match applied {
                Ok(_) => {
                    let cleanup = conn.execute(
                        &format!("DELETE FROM kv_{JOURNAL_NAMESPACE} WHERE key = ?1"),
                        rusqlite::params![key],
                    );
                    match cleanup {
                        Ok(_) => report.reapplied += 1,
                        Err(e) => {
                            tracing::error!(op_id = %key, %e, "sahne satiri silinemedi");
                            report.failed += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(op_id = %key, %e, "niyet geri uygulanamadi");
                    report.failed += 1;
                }
            }
        }

        Ok(report)
    }

    // -- ic yardimcilar -----------------------------------------------------

    /// Buyuk JSON'u CAS'a tasir, tabloda `{"cas":"<hash>"}` birakir (14.3).
    async fn maybe_offload(
        &self,
        json: String,
        refs: &mut Vec<String>,
    ) -> Result<String, StorageError> {
        if json.len() <= self.inline_limit {
            return Ok(json);
        }
        let hash = BlobStore::put(&self.cas, json.as_bytes()).await?;
        refs.push(hash.clone());
        serde_json::to_string(&serde_json::json!({ "cas": hash }))
            .map_err(|e| StorageError::Serialization(format!("cas isaretcisi: {e}")))
    }

    async fn store_blob(
        &self,
        data: Option<Vec<u8>>,
        refs: &mut Vec<String>,
    ) -> Result<Option<String>, StorageError> {
        match data {
            Some(bytes) => {
                let hash = BlobStore::put(&self.cas, &bytes).await?;
                refs.push(hash.clone());
                Ok(Some(hash))
            }
            None => Ok(None),
        }
    }

    /// Ajan basina monoton `seq`. Ilk dokunusta tablodaki en buyuk degerden
    /// devam eder; sonrasi bellekten sayilir.
    fn allocate_seq(&self, agent_id: i64) -> Result<i64, StorageError> {
        if let Some(mut slot) = self.next_seq.get_mut(&agent_id) {
            let seq = *slot;
            *slot = seq + 1;
            return Ok(seq);
        }

        let start = {
            let conn = self.read_conn.lock();
            conn.query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM agent_events WHERE agent_id = ?1",
                rusqlite::params![agent_id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| StorageError::Internal(format!("seq baslangici okunamadi: {e}")))?
        };

        let mut slot = self.next_seq.entry(agent_id).or_insert(start);
        let seq = *slot;
        *slot = seq + 1;
        Ok(seq)
    }

    /// I7: once niyet (`applied=0`), sonra asil satir + `applied=1` tek islemde.
    async fn journal_and_apply(
        &self,
        write: ResolvedWrite,
        refs: Vec<String>,
    ) -> Result<WriteReceipt, StorageError> {
        let op_id = Uuid::new_v4().to_string();
        let ts = chrono::Utc::now().to_rfc3339();
        let seq = write.seq();

        let intent = JournalIntent {
            op_id: op_id.clone(),
            ts: ts.clone(),
            write,
        };
        let payload = serde_json::to_vec(&intent)
            .map_err(|e| StorageError::Serialization(format!("niyet govdesi: {e}")))?;

        // 1) Niyet kaydi — yan etkiden ONCE ve tek basina taahhut edilir.
        self.exec_one(
            self.journal.sql.clone(),
            self.journal.params(&op_id, &payload),
        )
        .await?;

        // 2) Asil satir + applied=1: uzunluk > 1 oldugu icin WriterActor bunu
        //    BEGIN IMMEDIATE / COMMIT icine alir.
        let (sql, params) = intent.write.insert_sql(&ts);
        self.exec_batch(vec![
            (sql, params),
            (
                "UPDATE write_journal SET applied = 1 WHERE op_id = ?1".to_string(),
                vec![Value::Text(op_id.clone())],
            ),
        ])
        .await?;

        Ok(WriteReceipt {
            op_id,
            ts,
            seq,
            refs,
        })
    }

    async fn exec_one(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> Result<usize, StorageError> {
        let (reply, rx) = oneshot::channel();
        self.writer
            .write(WriteOp::Execute { sql, params, reply })
            .await?;
        rx.await
            .map_err(|_| StorageError::Internal("yazici yaniti dustu".into()))?
    }

    async fn exec_batch(
        &self,
        ops: Vec<(String, Vec<Value>)>,
    ) -> Result<(), StorageError> {
        let mut writes = Vec::with_capacity(ops.len());
        let mut waiters = Vec::with_capacity(ops.len());
        for (sql, params) in ops {
            let (reply, rx) = oneshot::channel();
            writes.push(WriteOp::Execute { sql, params, reply });
            waiters.push(rx);
        }

        self.writer.write(WriteOp::Batch { ops: writes }).await?;

        for rx in waiters {
            rx.await
                .map_err(|_| StorageError::Internal("yazici yaniti dustu".into()))??;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sema yardimcilari
// ---------------------------------------------------------------------------

fn open_db(db_path: &Path) -> Result<rusqlite::Connection, StorageError> {
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| StorageError::Internal(format!("db dizini olusturulamadi: {e}")))?;
    }
    JournalMode::for_db_path(db_path)
        .open(db_path)
        .map_err(|e| StorageError::Internal(format!("db acilamadi: {e}")))
}

/// `write_journal` 0007 semasiyla kurulmus olabilir; `WalReplay` bekledigi
/// sutunlari bulamazsa hicbir niyet oynatilamaz. Eksik sutunlari ekleriz —
/// `ALTER TABLE ADD COLUMN` gerektigi icin hepsi nullable/varsayilanlidir.
fn align_journal_schema(conn: &rusqlite::Connection) -> Result<(), StorageError> {
    let present = table_columns(conn, "write_journal")?;
    let wanted: &[(&str, &str)] = &[
        ("namespace", "TEXT"),
        ("key", "TEXT"),
        ("value", "BLOB"),
        ("op_type", "TEXT DEFAULT 'SET'"),
    ];

    for (name, decl) in wanted {
        if present.iter().any(|c| c == name) {
            continue;
        }
        conn.execute_batch(&format!(
            "ALTER TABLE write_journal ADD COLUMN \"{name}\" {decl}"
        ))
        .map_err(|e| {
            StorageError::Internal(format!("write_journal.{name} eklenemedi: {e}"))
        })?;
        tracing::info!(column = name, "write_journal sutunu eklendi");
    }
    Ok(())
}

/// `WalReplay::apply_op` niyetleri `kv_{namespace}` tablosuna yazar; hedef
/// yoksa replay basarisiz sayilir. Sahne tablosunu acilista kurariz.
fn ensure_replay_staging(conn: &rusqlite::Connection) -> Result<(), StorageError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS kv_{JOURNAL_NAMESPACE} \
         (key TEXT PRIMARY KEY, value BLOB)"
    ))
    .map_err(|e| StorageError::Internal(format!("sahne tablosu kurulamadi: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SCHEMA: &str = "
        CREATE TABLE IF NOT EXISTS agent_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id INTEGER NOT NULL,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT,
            ts TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id INTEGER NOT NULL,
            role TEXT NOT NULL,
            provider_model TEXT,
            content_ref TEXT,
            tokens_in INTEGER,
            tokens_out INTEGER,
            cost REAL,
            ts TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS tool_calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id INTEGER NOT NULL,
            tool TEXT NOT NULL,
            args_json TEXT,
            result_ref TEXT,
            status TEXT NOT NULL,
            capability_ok INTEGER,
            ts TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS file_touches (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            outside_workspace INTEGER NOT NULL DEFAULT 0,
            added INTEGER NOT NULL DEFAULT 0,
            removed INTEGER NOT NULL DEFAULT 0,
            pre_ref TEXT,
            post_ref TEXT,
            ts TEXT NOT NULL DEFAULT (datetime('now'))
        );
    ";

    struct Harness {
        _dir: TempDir,
        db: PathBuf,
        writer: EventWriter,
    }

    fn setup() -> Result<Harness, StorageError> {
        let dir = TempDir::new()
            .map_err(|e| StorageError::Internal(format!("tempdir: {e}")))?;
        let db = EventWriter::effective_db_path(&dir.path().join("omni.db"));
        {
            let conn = open_db(&db)?;
            conn.execute_batch(SCHEMA)
                .map_err(|e| StorageError::Internal(e.to_string()))?;
        }
        let cas = CasBlobStore::new(&dir.path().join("cas"))?;
        let actor = Arc::new(WriterActor::new(&db));
        let writer = EventWriter::with_writer(&db, cas, actor)?;
        Ok(Harness {
            _dir: dir,
            db,
            writer,
        })
    }

    fn count(db: &Path, table: &str) -> Result<i64, StorageError> {
        let conn = open_db(db)?;
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .map_err(|e| StorageError::Internal(e.to_string()))
    }

    #[tokio::test]
    async fn olay_yazimi_seq_uretir() -> Result<(), StorageError> {
        let h = setup()?;
        let a = h
            .writer
            .record_event(AgentEventRecord {
                agent_id: 1,
                kind: "turn_started".into(),
                payload_json: Some("{\"n\":1}".into()),
            })
            .await?;
        let b = h
            .writer
            .record_event(AgentEventRecord {
                agent_id: 1,
                kind: "turn_ended".into(),
                payload_json: None,
            })
            .await?;

        assert_eq!(a.seq, Some(0));
        assert_eq!(b.seq, Some(1));

        h.writer.writer.flush().await;
        assert_eq!(count(&h.db, "agent_events")?, 2);
        Ok(())
    }

    #[tokio::test]
    async fn niyet_kaydi_applied_olarak_kapanir() -> Result<(), StorageError> {
        let h = setup()?;
        let receipt = h
            .writer
            .record_event(AgentEventRecord {
                agent_id: 7,
                kind: "tick".into(),
                payload_json: None,
            })
            .await?;
        h.writer.writer.flush().await;

        let conn = open_db(&h.db)?;
        let applied: i64 = conn
            .query_row(
                "SELECT applied FROM write_journal WHERE op_id = ?1",
                rusqlite::params![receipt.op_id],
                |r| r.get(0),
            )
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        assert_eq!(applied, 1);
        Ok(())
    }

    #[tokio::test]
    async fn mesaj_icerigi_cas_referansi_olur() -> Result<(), StorageError> {
        let h = setup()?;
        let receipt = h
            .writer
            .record_message(MessageRecord {
                agent_id: 3,
                role: "assistant".into(),
                provider_model: Some("test-provider/test-alias".into()),
                content: b"merhaba".to_vec(),
                tokens_in: Some(10),
                tokens_out: Some(20),
                cost: Some(0.0),
            })
            .await?;
        h.writer.writer.flush().await;

        assert_eq!(receipt.refs.len(), 1);
        let conn = open_db(&h.db)?;
        let stored: String = conn
            .query_row("SELECT content_ref FROM messages", [], |r| r.get(0))
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        let blob = h.writer.cas().load(&stored)?;
        assert_eq!(blob.as_deref(), Some(&b"merhaba"[..]));
        Ok(())
    }

    #[tokio::test]
    async fn buyuk_payload_cas_isaretcisine_doner() -> Result<(), StorageError> {
        let h = setup()?;
        let writer = EventWriter::with_writer(
            &h.db,
            h.writer.cas().clone(),
            Arc::clone(&h.writer.writer),
        )?
        .with_inline_limit(8);

        let receipt = writer
            .record_tool_call(ToolCallRecord {
                agent_id: 4,
                tool: "edit".into(),
                args_json: Some("{\"path\":\"cok/uzun/bir/yol\"}".into()),
                result: Some(b"tamam".to_vec()),
                status: "ok".into(),
                capability_ok: Some(true),
            })
            .await?;
        writer.writer.flush().await;

        // biri args_json offload'i, digeri result_ref.
        assert_eq!(receipt.refs.len(), 2);
        let conn = open_db(&h.db)?;
        let args: String = conn
            .query_row("SELECT args_json FROM tool_calls", [], |r| r.get(0))
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        assert!(args.contains("cas"), "args_json cas isaretcisi olmali: {args}");
        Ok(())
    }

    #[tokio::test]
    async fn dosya_dokunusu_pre_post_saklar() -> Result<(), StorageError> {
        let h = setup()?;
        h.writer
            .record_file_touch(FileTouchRecord {
                agent_id: 5,
                path: "/calisma/disi/dosya.rs".into(),
                outside_workspace: true,
                added: 12,
                removed: 3,
                pre: Some(b"eski".to_vec()),
                post: Some(b"yeni".to_vec()),
            })
            .await?;
        h.writer.writer.flush().await;

        let conn = open_db(&h.db)?;
        let (outside, added, removed): (i64, i64, i64) = conn
            .query_row(
                "SELECT outside_workspace, added, removed FROM file_touches",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| StorageError::Internal(e.to_string()))?;
        assert_eq!((outside, added, removed), (1, 12, 3));
        Ok(())
    }

    #[tokio::test]
    async fn yarim_kalan_niyet_kurtarilir() -> Result<(), StorageError> {
        let h = setup()?;

        // Cokme benzetimi: niyet yazildi, asil satir yazilmadan surec olduysa
        // write_journal'da applied=0 satir kalir.
        let intent = JournalIntent {
            op_id: "op-kurtarma".into(),
            ts: "2026-01-01T00:00:00+00:00".into(),
            write: ResolvedWrite::AgentEvent {
                agent_id: 9,
                seq: 0,
                kind: "yarim".into(),
                payload_json: None,
            },
        };
        let payload = serde_json::to_vec(&intent)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        h.writer
            .exec_one(
                h.writer.journal.sql.clone(),
                h.writer.journal.params(&intent.op_id, &payload),
            )
            .await?;
        h.writer.writer.flush().await;

        assert_eq!(count(&h.db, "agent_events")?, 0);

        let report = h.writer.recover()?;
        assert_eq!(report.replayed, 1, "niyet sahne tablosuna tasinmali");
        assert_eq!(report.reapplied, 1, "niyet asil tabloya yazilmali");
        assert_eq!(report.failed, 0);
        assert_eq!(count(&h.db, "agent_events")?, 1);

        // Ikinci kurtarma bos gecmeli (idempotent).
        let again = h.writer.recover()?;
        assert_eq!(again.replayed, 0);
        assert_eq!(again.reapplied, 0);
        assert_eq!(count(&h.db, "agent_events")?, 1);
        Ok(())
    }

    #[test]
    fn eski_journal_semasi_hizalanir() -> Result<(), StorageError> {
        let dir = TempDir::new()
            .map_err(|e| StorageError::Internal(format!("tempdir: {e}")))?;
        let db = EventWriter::effective_db_path(&dir.path().join("eski.db"));
        let conn = open_db(&db)?;
        // 0007 semasi
        conn.execute_batch(
            "CREATE TABLE write_journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                op_id TEXT NOT NULL UNIQUE,
                op_kind TEXT NOT NULL,
                payload_ref TEXT,
                applied INTEGER NOT NULL DEFAULT 0,
                ts TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .map_err(|e| StorageError::Internal(e.to_string()))?;

        align_journal_schema(&conn)?;
        let cols = table_columns(&conn, "write_journal")?;
        for want in ["namespace", "key", "value", "op_type", "op_kind"] {
            assert!(cols.iter().any(|c| c == want), "{want} sutunu olmali");
        }

        let layout = JournalLayout::detect(&conn)?;
        assert!(layout.cols.contains(&JournalCol::OpKind));
        let params = layout.params("op-1", b"govde");
        assert_eq!(params.len(), layout.cols.len());
        Ok(())
    }
}
