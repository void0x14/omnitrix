//! omni-research — saglayici-degistirilebilir arastirma motoru
//! (MASTER-PLAN 3.2 YENI, 19.2, 6.2, K14; Faz 7).
//!
//! # Sozlesme
//!
//! * **Cekirdek yalnizca trait'i bilir.** [`ResearchEngine`] tarama dongusunu
//!   [`provider::ResearchProvider`] uzerinden yurutur. Bugun bu trait'i hazir
//!   MCP tasimasi uygular ([`provider::McpResearchProvider`], `xai-grok-mcp`
//!   uzerinden Firecrawl/Exa + anti-detect); yarin native crawler geldiginde
//!   ikinci bir uygulama yazilir ve **bu dosya degismez**. Kanit:
//!   `saglayici_degisince_cekirdek_degismez` testi ayni `ResearchEngine`
//!   ornegini iki farkli saglayiciyla calistirir.
//! * **Modlar** (19.2): yuzeysel / derin / okyanus. Butceler [`modes`] icinde
//!   somut sayilarla durur.
//! * **Cikti** (19.2): JSON **kanonik**, Markdown **turev**. Ikisi de ayni
//!   [`ResearchReport`] degerinden uretilir; Markdown asla ayri bir gercek
//!   kaynagi degildir.
//! * **Kalicilik** (6.2, 14.3): her iki bicim de `research_findings` tablosuna
//!   birer satir olur; govde CAS'a gider, tabloda yalnizca `result_ref` (icerik
//!   hash'i) durur.
//! * **I7**: her satir yaziminin **oncesinde** `write_journal`'a `applied=0`
//!   niyet kaydi dusulur; asil INSERT + `applied=1` tek batch icinde gider.
//! * **I6**: uretim yolunda `unwrap`/`expect`/`panic!` yoktur; her sey
//!   [`ResearchError`] ile tasinir.

pub mod modes;
pub mod provider;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use omni_proto::{TaskId, Timestamp};
use omni_storage::events::EventWriter;
use omni_storage::traits::{BlobStore, StorageError};
use omni_storage::wal::WalReplay;
use omni_storage::writer_actor::{WriteOp, WriterActor};

pub use modes::{ModeParams, ResearchMode};
pub use provider::{ProviderConfig, ResearchProvider, SearchRequest};

use omni_storage::cas::CasBlobStore;

/// Niyet kayitlarinin `write_journal.namespace` degeri. Replay hedefi bundan
/// turetilen `kv_omni_research` tablosudur.
const JOURNAL_NAMESPACE: &str = "omni_research";

/// `write_journal.op_kind` sutunu 0007 semasinda `NOT NULL`; o sema yuruyorsa
/// bu deger yazilir.
const JOURNAL_OP_KIND: &str = "omni_research_finding";

/// Markdown turevinde tam icerikten gosterilecek karakter ust siniri.
const MD_CONTENT_CHARS: usize = 1200;

// ---------------------------------------------------------------------------
// Hata
// ---------------------------------------------------------------------------

/// Arastirma katmani hatalari (I6).
#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    /// Saglayici cagrisi basarisiz oldu.
    #[error("saglayici hatasi ({provider}): {message}")]
    Provider {
        /// Hatayi ureten saglayicinin adi.
        provider: String,
        /// Saglayicidan gelen mesaj.
        message: String,
    },
    /// Saglayici yapilandirmasi gecersiz.
    #[error("saglayici yapilandirmasi gecersiz: {0}")]
    Config(String),
    /// Bos sorgu gibi cagrim hatalari.
    #[error("gecersiz istek: {0}")]
    Invalid(String),
    /// Depolama katmani hatasi.
    #[error("depolama hatasi: {0}")]
    Storage(#[from] StorageError),
    /// JSON kodlama/cozme hatasi.
    #[error("JSON hatasi: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Veri modeli
// ---------------------------------------------------------------------------

/// Tek bir kaynak bulgusu. Kanonik JSON ciktisinin atomu budur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Kaynak adresi. Yapisiz saglayici yanitlarinda bos olabilir.
    pub url: String,
    /// Baslik.
    pub title: String,
    /// Kisa ozet.
    pub snippet: String,
    /// Tam icerik (varsa).
    pub content: Option<String>,
    /// Saglayicinin verdigi ilgi skoru; yoksa 0.
    pub score: f64,
    /// Bulguyu ureten saglayicinin adi.
    pub source: String,
    /// Kacinci genisletme turunda bulundu.
    pub round: u8,
    /// Alinma ani.
    pub fetched_at: Timestamp,
}

impl Finding {
    /// Tekilleme anahtari: URL varsa normalize edilmis URL, yoksa baslik+ozet.
    #[must_use]
    pub fn dedup_key(&self) -> String {
        if self.url.trim().is_empty() {
            format!("t:{}|{}", self.title.trim(), self.snippet.trim())
        } else {
            format!("u:{}", normalize_url(&self.url))
        }
    }
}

/// Sondaki `/`, sema ve `www.` farklarini silen kaba normalizasyon.
fn normalize_url(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let without_www = without_scheme.strip_prefix("www.").unwrap_or(without_scheme);
    without_www.trim_end_matches('/').to_string()
}

/// `research_findings.format` degeri. JSON kanonik, Markdown turevdir (19.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchFormat {
    /// Kanonik JSON govde.
    Json,
    /// JSON'dan turetilmis Markdown.
    Markdown,
}

impl ResearchFormat {
    /// DB'ye yazilan deger.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            ResearchFormat::Json => "json",
            ResearchFormat::Markdown => "md",
        }
    }

    /// DB degerinden cozer.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "json" => Some(ResearchFormat::Json),
            "md" | "markdown" => Some(ResearchFormat::Markdown),
            _ => None,
        }
    }
}

/// Bir arastirmanin tam sonucu. **Kanonik gercek kaynagi** budur; Markdown
/// bundan uretilir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchReport {
    /// Baglandigi gorev (`research_findings.task_id`).
    pub task_id: TaskId,
    /// Tarama modu.
    pub mode: ResearchMode,
    /// Kullanicinin verdigi kok sorgu.
    pub query: String,
    /// Sonucu ureten saglayicinin adi.
    pub provider: String,
    /// Rapor ani.
    pub generated_at: Timestamp,
    /// Fiilen calisan tur sayisi.
    pub rounds_run: u8,
    /// Fiilen calistirilan tum sorgular (kok sorgu dahil, sirali).
    pub queries: Vec<String>,
    /// Tekillenmis, skora gore sirali bulgular.
    pub findings: Vec<Finding>,
    /// Butce dolduğu icin sonuc kesildi mi.
    pub truncated: bool,
}

impl ResearchReport {
    /// Kanonik JSON govdesi.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, ResearchError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Markdown turevi — insan okunur, ayni veriden uretilir (19.2).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::with_capacity(1024 + self.findings.len() * 256);

        let _ = writeln!(out, "# Arastirma: {}", inline(&self.query));
        out.push('\n');
        let _ = writeln!(out, "- **Mod:** `{}`", self.mode.as_db_str());
        let _ = writeln!(out, "- **Gorev:** {}", self.task_id);
        let _ = writeln!(out, "- **Saglayici:** {}", inline(&self.provider));
        let _ = writeln!(
            out,
            "- **Uretim:** {}",
            self.generated_at.to_rfc3339()
        );
        let _ = writeln!(
            out,
            "- **Tur:** {} · **Sorgu:** {} · **Bulgu:** {}",
            self.rounds_run,
            self.queries.len(),
            self.findings.len()
        );
        let _ = writeln!(
            out,
            "- **Butce doldu:** {}",
            if self.truncated { "evet" } else { "hayir" }
        );

        out.push_str("\n## Calistirilan sorgular\n\n");
        for (i, q) in self.queries.iter().enumerate() {
            let _ = writeln!(out, "{}. `{}`", i + 1, inline(q));
        }

        out.push_str("\n## Bulgular\n");
        if self.findings.is_empty() {
            out.push_str("\n_Bulgu yok._\n");
            return out;
        }

        for (i, f) in self.findings.iter().enumerate() {
            let _ = writeln!(out, "\n### {}. {}", i + 1, inline(&f.title));
            if f.url.is_empty() {
                let _ = writeln!(out, "\n- **Kaynak:** {} (URL yok)", inline(&f.source));
            } else {
                let _ = writeln!(out, "\n- **URL:** <{}>", inline(&f.url));
                let _ = writeln!(out, "- **Kaynak:** {}", inline(&f.source));
            }
            let _ = writeln!(out, "- **Skor:** {:.3} · **Tur:** {}", f.score, f.round);
            if !f.snippet.trim().is_empty() {
                let _ = writeln!(out, "\n{}", inline(&f.snippet));
            }
            if let Some(content) = &f.content
                && !content.trim().is_empty()
            {
                let shown = take_chars(content, MD_CONTENT_CHARS);
                let elided = shown.chars().count() < content.chars().count();
                out.push_str("\n```text\n");
                out.push_str(&shown);
                if elided {
                    out.push_str("\n… (kisaltildi, tam govde JSON kanonikte)");
                }
                out.push_str("\n```\n");
            }
        }
        out
    }
}

/// Satir ici markdown icin: satir sonlarini bosluga cevirir, ` ` kacar.
fn inline(s: &str) -> String {
    s.replace(['\r', '\n'], " ").replace('`', "'")
}

fn take_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Kalicilik — research_findings + CAS + I7 niyet kaydi
// ---------------------------------------------------------------------------

/// `research_findings`'e yazilacak tek satir.
#[derive(Debug, Clone)]
pub struct FindingRecord {
    /// Gorev kimligi.
    pub task_id: TaskId,
    /// Tarama modu.
    pub mode: ResearchMode,
    /// Kok sorgu.
    pub query: String,
    /// Govde bicimi.
    pub format: ResearchFormat,
    /// Govde. Daima CAS'a gider (14.3); tabloda yalnizca hash durur.
    pub body: Vec<u8>,
}

/// Tek satir yaziminin makbuzu.
#[derive(Debug, Clone)]
pub struct FindingReceipt {
    /// Niyet kaydinin `op_id`'si.
    pub op_id: String,
    /// Satirin `ts` degeri (RFC 3339).
    pub ts: String,
    /// CAS icerik hash'i (`research_findings.result_ref`).
    pub result_ref: String,
    /// Yazilan bicim.
    pub format: ResearchFormat,
}

/// Niyet govdesi: replay sirasinda hicbir yan etki tekrar uretilmesin diye
/// CAS ref'i **cozulmus** halde saklanir.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FindingIntent {
    op_id: String,
    ts: String,
    task_id: TaskId,
    mode: String,
    query: String,
    result_ref: String,
    format: String,
}

impl FindingIntent {
    /// Idempotent INSERT (8.1 adim 4): ayni niyet tekrar oynatilsa da ikinci
    /// satir olusmaz.
    fn insert_sql(&self) -> (String, Vec<SqlValue>) {
        (
            "INSERT INTO research_findings (task_id, mode, query, result_ref, format, ts) \
             SELECT ?1, ?2, ?3, ?4, ?5, ?6 \
             WHERE NOT EXISTS (SELECT 1 FROM research_findings \
             WHERE task_id = ?1 AND format = ?5 AND ts = ?6 AND result_ref = ?4)"
                .to_string(),
            vec![
                SqlValue::Integer(self.task_id),
                SqlValue::Text(self.mode.clone()),
                SqlValue::Text(self.query.clone()),
                SqlValue::Text(self.result_ref.clone()),
                SqlValue::Text(self.format.clone()),
                SqlValue::Text(self.ts.clone()),
            ],
        )
    }
}

/// `write_journal` iki ayri semayla var olabilir (0007 vs `wal.rs`). Hangi
/// sutunlarin bulundugunu acilista tespit edip INSERT'i ona gore kurariz.
#[derive(Debug, Clone)]
struct JournalLayout {
    cols: Vec<&'static str>,
    sql: String,
}

impl JournalLayout {
    fn detect(conn: &rusqlite::Connection) -> Result<Self, StorageError> {
        let present = table_columns(conn, "write_journal")?;
        let mut cols: Vec<&'static str> = vec!["op_id"];
        for name in ["namespace", "key", "value", "op_type", "op_kind", "payload_ref"] {
            if present.iter().any(|c| c == name) {
                cols.push(name);
            }
        }

        let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT OR IGNORE INTO write_journal ({}, applied) VALUES ({}, 0)",
            cols.join(", "),
            placeholders.join(", ")
        );
        Ok(Self { cols, sql })
    }

    fn params(&self, op_id: &str, payload: &[u8]) -> Vec<SqlValue> {
        self.cols
            .iter()
            .map(|c| match *c {
                "namespace" => SqlValue::Text(JOURNAL_NAMESPACE.to_string()),
                "value" => SqlValue::Blob(payload.to_vec()),
                "op_type" => SqlValue::Text("SET".to_string()),
                "op_kind" => SqlValue::Text(JOURNAL_OP_KIND.to_string()),
                "payload_ref" => SqlValue::Null,
                // "op_id" ve "key" ayni degeri tasir.
                _ => SqlValue::Text(op_id.to_string()),
            })
            .collect()
    }
}

fn table_columns(conn: &rusqlite::Connection, table: &str) -> Result<Vec<String>, StorageError> {
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

/// `research_findings` yazicisi.
///
/// Yazim sirasi (I7):
/// 1. Govde CAS'a konur, `result_ref` bellidir.
/// 2. `write_journal`'a `applied=0` niyet kaydi **tek basina** taahhut edilir.
/// 3. Asil INSERT + `applied=1` guncellemesi tek `WriteOp::Batch` icinde gider;
///    `WriterActor` uzunlugu 1'den buyuk batch'i `BEGIN IMMEDIATE`/`COMMIT` ile
///    sarar — ya ikisi de olur ya hicbiri.
pub struct FindingsSink {
    db_path: PathBuf,
    writer: Arc<WriterActor>,
    cas: CasBlobStore,
    journal: JournalLayout,
}

impl FindingsSink {
    /// Verilmis bir [`WriterActor`] uzerine kurar. `db_path`
    /// [`EventWriter::effective_db_path`] ile normalize edilmis olmalidir.
    pub fn with_writer(
        db_path: &Path,
        cas: CasBlobStore,
        writer: Arc<WriterActor>,
    ) -> Result<Self, ResearchError> {
        let path = EventWriter::effective_db_path(db_path);
        let conn = rusqlite::Connection::open(&path)
            .map_err(|e| StorageError::Internal(format!("db acilamadi: {e}")))?;

        WalReplay::ensure_write_journal(&conn)?;
        align_journal_schema(&conn)?;
        ensure_replay_staging(&conn)?;
        let journal = JournalLayout::detect(&conn)?;

        Ok(Self {
            db_path: path,
            writer,
            cas,
            journal,
        })
    }

    /// Kendi [`WriterActor`]'unu kurar. Tokio runtime icinde cagrilmalidir.
    pub fn open(db_path: &Path, cas_base: &Path) -> Result<Self, ResearchError> {
        let path = EventWriter::effective_db_path(db_path);
        let cas = CasBlobStore::new(cas_base)?;
        let writer = Arc::new(WriterActor::new(&path));
        Self::with_writer(&path, cas, writer)
    }

    /// Kullanilan DB dosyasi.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Govdelerin saklandigi CAS.
    #[must_use]
    pub fn cas(&self) -> &CasBlobStore {
        &self.cas
    }

    /// Tek satir yazar (I7 sirasiyla).
    pub async fn record(&self, rec: FindingRecord) -> Result<FindingReceipt, ResearchError> {
        let result_ref = BlobStore::put(&self.cas, &rec.body).await?;
        let op_id = uuid::Uuid::new_v4().to_string();
        let ts = chrono::Utc::now().to_rfc3339();

        let intent = FindingIntent {
            op_id: op_id.clone(),
            ts: ts.clone(),
            task_id: rec.task_id,
            mode: rec.mode.as_db_str().to_string(),
            query: rec.query,
            result_ref: result_ref.clone(),
            format: rec.format.as_db_str().to_string(),
        };
        let payload = serde_json::to_vec(&intent)?;

        // 1) Niyet — yan etkiden ONCE, tek basina.
        self.exec_one(
            self.journal.sql.clone(),
            self.journal.params(&op_id, &payload),
        )
        .await?;

        // 2) Asil satir + applied=1 (tek batch).
        let (sql, params) = intent.insert_sql();
        self.exec_batch(vec![
            (sql, params),
            (
                "UPDATE write_journal SET applied = 1 WHERE op_id = ?1".to_string(),
                vec![SqlValue::Text(op_id.clone())],
            ),
        ])
        .await?;

        Ok(FindingReceipt {
            op_id,
            ts,
            result_ref,
            format: rec.format,
        })
    }

    /// CAS'tan govde okur (`result_ref` -> icerik).
    pub async fn load_body(&self, result_ref: &str) -> Result<Option<Vec<u8>>, ResearchError> {
        Ok(BlobStore::get(&self.cas, result_ref).await?)
    }

    /// Acilis kurtarmasi: `WalReplay` yarim kalan niyetleri sahne tablosuna
    /// tasir, buradaki dongu onlari asil tabloya idempotent geri uygular.
    /// Donen sayi geri uygulanan niyet adedidir.
    pub fn recover(&self) -> Result<u64, ResearchError> {
        let replayed = WalReplay::new(&self.db_path)?.replay_pending()?;
        tracing::debug!(replayed = replayed.replayed, "arastirma niyetleri oynatildi");

        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| StorageError::Internal(format!("db acilamadi: {e}")))?;
        ensure_replay_staging(&conn)?;

        let staged: Vec<(String, Vec<u8>)> = {
            let mut stmt = conn
                .prepare(&format!("SELECT key, value FROM kv_{JOURNAL_NAMESPACE}"))
                .map_err(|e| StorageError::Internal(format!("sahne sorgusu: {e}")))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))
                .map_err(|e| StorageError::Internal(format!("sahne okuma: {e}")))?;
            let mut out = Vec::new();
            for row in rows {
                match row {
                    Ok(v) => out.push(v),
                    Err(e) => tracing::warn!(%e, "sahne satiri okunamadi, atlaniyor"),
                }
            }
            out
        };

        let mut reapplied = 0u64;
        for (key, payload) in staged {
            let intent: FindingIntent = match serde_json::from_slice(&payload) {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!(op_id = %key, %e, "niyet govdesi cozulemedi");
                    continue;
                }
            };
            let (sql, params) = intent.insert_sql();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|v| v as &dyn rusqlite::types::ToSql)
                .collect();
            match conn.execute(&sql, rusqlite::params_from_iter(param_refs.iter().copied())) {
                Ok(_) => {
                    match conn.execute(
                        &format!("DELETE FROM kv_{JOURNAL_NAMESPACE} WHERE key = ?1"),
                        rusqlite::params![key],
                    ) {
                        Ok(_) => reapplied += 1,
                        Err(e) => tracing::error!(op_id = %key, %e, "sahne satiri silinemedi"),
                    }
                }
                Err(e) => tracing::error!(op_id = %key, %e, "niyet geri uygulanamadi"),
            }
        }
        Ok(reapplied)
    }

    async fn exec_one(&self, sql: String, params: Vec<SqlValue>) -> Result<usize, ResearchError> {
        let (reply, rx) = oneshot::channel();
        self.writer.write(WriteOp::Execute { sql, params, reply }).await?;
        let n = rx
            .await
            .map_err(|_| StorageError::Internal("yazici yaniti dustu".into()))??;
        Ok(n)
    }

    async fn exec_batch(&self, ops: Vec<(String, Vec<SqlValue>)>) -> Result<(), ResearchError> {
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

/// 0007 semasi `namespace`/`key`/`value`/`op_type` sutunlarini tasimaz;
/// `WalReplay` onlar olmadan hicbir niyeti oynatamaz. Eksikleri ekleriz
/// (`ALTER TABLE ADD COLUMN` gerektigi icin hepsi nullable/varsayilanli).
fn align_journal_schema(conn: &rusqlite::Connection) -> Result<(), StorageError> {
    let present = table_columns(conn, "write_journal")?;
    for (name, decl) in [
        ("namespace", "TEXT"),
        ("key", "TEXT"),
        ("value", "BLOB"),
        ("op_type", "TEXT DEFAULT 'SET'"),
    ] {
        if present.iter().any(|c| c == name) {
            continue;
        }
        conn.execute_batch(&format!(
            "ALTER TABLE write_journal ADD COLUMN \"{name}\" {decl}"
        ))
        .map_err(|e| StorageError::Internal(format!("write_journal.{name} eklenemedi: {e}")))?;
    }
    Ok(())
}

/// `WalReplay::apply_op` niyetleri `kv_{namespace}` tablosuna yazar; hedef
/// yoksa replay basarisiz sayilir.
fn ensure_replay_staging(conn: &rusqlite::Connection) -> Result<(), StorageError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS kv_{JOURNAL_NAMESPACE} (key TEXT PRIMARY KEY, value BLOB)"
    ))
    .map_err(|e| StorageError::Internal(format!("sahne tablosu kurulamadi: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cekirdek: ResearchEngine
// ---------------------------------------------------------------------------

/// Bir arastirmanin sonucu: kanonik rapor + iki satirin makbuzu.
#[derive(Debug, Clone)]
pub struct ResearchOutcome {
    /// Kanonik rapor.
    pub report: ResearchReport,
    /// JSON satirinin makbuzu.
    pub json: FindingReceipt,
    /// Markdown satirinin makbuzu.
    pub markdown: FindingReceipt,
}

/// Saglayicidan bagimsiz tarama dongusu.
///
/// Bu tip `dyn ResearchProvider` tutar; hangi saglayicinin takili oldugunu
/// bilmez. Saglayici config'ten degistiginde ([`ProviderConfig::build`]) bu
/// dosyada tek satir degismez — kapinin kanit noktasi budur.
pub struct ResearchEngine {
    provider: Arc<dyn ResearchProvider>,
    sink: Arc<FindingsSink>,
}

impl ResearchEngine {
    /// Saglayici + yazici ile motor kurar.
    #[must_use]
    pub fn new(provider: Arc<dyn ResearchProvider>, sink: Arc<FindingsSink>) -> Self {
        Self { provider, sink }
    }

    /// Config'ten saglayici cozerek motor kurar (AS8).
    pub fn from_config(
        cfg: &ProviderConfig,
        sink: Arc<FindingsSink>,
    ) -> Result<Self, ResearchError> {
        Ok(Self::new(cfg.build()?, sink))
    }

    /// Takili saglayicinin adi.
    #[must_use]
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    /// Kalicilik katmani.
    #[must_use]
    pub fn sink(&self) -> &Arc<FindingsSink> {
        &self.sink
    }

    /// Verilen modda arastirir, raporu uretir ve **iki bicimi de**
    /// `research_findings`'e yazar (kapi).
    pub async fn investigate(
        &self,
        task_id: TaskId,
        query: &str,
        mode: ResearchMode,
    ) -> Result<ResearchOutcome, ResearchError> {
        let report = self.collect(task_id, query, mode).await?;

        let json = self
            .sink
            .record(FindingRecord {
                task_id,
                mode,
                query: report.query.clone(),
                format: ResearchFormat::Json,
                body: report.to_json_bytes()?,
            })
            .await?;

        let markdown = self
            .sink
            .record(FindingRecord {
                task_id,
                mode,
                query: report.query.clone(),
                format: ResearchFormat::Markdown,
                body: report.to_markdown().into_bytes(),
            })
            .await?;

        Ok(ResearchOutcome {
            report,
            json,
            markdown,
        })
    }

    /// Tarama dongusu — kalicilik yok, yalnizca rapor uretir.
    pub async fn collect(
        &self,
        task_id: TaskId,
        query: &str,
        mode: ResearchMode,
    ) -> Result<ResearchReport, ResearchError> {
        let base = query.trim();
        if base.is_empty() {
            return Err(ResearchError::Invalid("sorgu bos".into()));
        }

        let params = mode.params();
        let mut findings: Vec<Finding> = Vec::new();
        let mut seen_urls: BTreeSet<String> = BTreeSet::new();
        let mut executed: Vec<String> = Vec::new();
        let mut pending: Vec<String> = vec![base.to_string()];
        let mut truncated = false;
        let mut rounds_run = 0u8;

        'rounds: for round in 0..params.refine_rounds {
            if pending.is_empty() {
                break;
            }
            rounds_run = round + 1;
            let batch = std::mem::take(&mut pending);

            for q in batch {
                if executed.len() >= params.max_queries {
                    truncated = true;
                    break 'rounds;
                }
                if executed.iter().any(|e| e == &q) {
                    continue;
                }
                if findings.len() >= params.max_sources {
                    truncated = true;
                    break 'rounds;
                }

                let req = SearchRequest {
                    query: q.clone(),
                    mode,
                    params,
                    round,
                };
                let batch_findings = self.provider.search(&req).await?;
                executed.push(q);

                for f in batch_findings.into_iter().take(params.per_query_results) {
                    if !seen_urls.insert(f.dedup_key()) {
                        continue;
                    }
                    if findings.len() >= params.max_sources {
                        truncated = true;
                        break;
                    }
                    findings.push(f);
                }
            }

            if round + 1 < params.refine_rounds && params.refine_fanout > 0 {
                pending = refine_queries(
                    base,
                    &findings,
                    usize::from(params.refine_fanout),
                    &executed,
                );
            }
        }

        // Skora gore azalan, esitlikte tur ve URL ile kararli siralama.
        findings.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.round.cmp(&b.round))
                .then_with(|| a.url.cmp(&b.url))
        });

        Ok(ResearchReport {
            task_id,
            mode,
            query: base.to_string(),
            provider: self.provider.name().to_string(),
            generated_at: omni_proto::now(),
            rounds_run,
            queries: executed,
            findings,
            truncated,
        })
    }
}

/// Bir turun bulgularindan sonraki turun sorgularini turetir.
///
/// Deterministiktir: baslik+ozet sozcukleri sayilir, kok sorguda gecmeyen ve
/// durak listesinde olmayan en sik `want` sozcuk kok sorgunun sonuna eklenir.
/// Model cagrisi yoktur — dongu saglayicidan da modelden de bagimsizdir.
#[must_use]
pub fn refine_queries(
    base: &str,
    findings: &[Finding],
    want: usize,
    executed: &[String],
) -> Vec<String> {
    if want == 0 || findings.is_empty() {
        return Vec::new();
    }

    let base_words: BTreeSet<String> = tokenize(base).into_iter().collect();
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for f in findings {
        for word in tokenize(&f.title).into_iter().chain(tokenize(&f.snippet)) {
            if base_words.contains(&word) || is_stopword(&word) {
                continue;
            }
            *counts.entry(word).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    // Sikliga gore azalan, esitlikte alfabetik: ayni girdi ayni ciktiyi verir.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut out = Vec::with_capacity(want);
    for (word, _) in ranked {
        if out.len() >= want {
            break;
        }
        let candidate = format!("{base} {word}");
        if executed.iter().any(|e| e == &candidate) || out.contains(&candidate) {
            continue;
        }
        out.push(candidate);
    }
    out
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "this"
            | "that"
            | "with"
            | "from"
            | "have"
            | "which"
            | "about"
            | "into"
            | "your"
            | "https"
            | "http"
            | "html"
            | "index"
            | "page"
            | "site"
            | "icin"
            | "olan"
            | "daha"
            | "gibi"
            | "veya"
    )
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// migrations/0003 + 0008'den birebir alinmis asgari sema.
    const SCHEMA: &str = "
        CREATE TABLE IF NOT EXISTS tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_id INTEGER REFERENCES tasks(id),
            root_id INTEGER NOT NULL REFERENCES tasks(id),
            title TEXT NOT NULL,
            mode TEXT NOT NULL,
            status TEXT NOT NULL,
            depth INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            closed_at TEXT
        );
        CREATE TABLE IF NOT EXISTS research_findings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES tasks(id),
            mode TEXT NOT NULL,
            query TEXT NOT NULL,
            result_ref TEXT,
            format TEXT,
            ts TEXT NOT NULL DEFAULT (datetime('now'))
        );
    ";

    /// Sahte saglayici: ag yok, deterministik. Cekirdegin saglayiciya bagli
    /// olmadigini kanitlamak icin trait'in ikinci bir uygulamasidir.
    struct FakeProvider {
        name: String,
        per_query: usize,
        calls: AtomicUsize,
    }

    impl FakeProvider {
        fn new(name: &str, per_query: usize) -> Self {
            Self {
                name: name.to_string(),
                per_query,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ResearchProvider for FakeProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn search(&self, req: &SearchRequest) -> Result<Vec<Finding>, ResearchError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let now = omni_proto::now();
            let out = (0..self.per_query)
                .map(|i| Finding {
                    url: format!("https://{}.invalid/{call}/{i}", self.name),
                    title: format!("{} sonuc {call}-{i} tokio kanal", self.name),
                    snippet: format!("{} icin ozet tokio kanal aktör", req.query),
                    content: Some(format!("govde {call}-{i}")),
                    score: 1.0 - (i as f64) / 100.0,
                    source: self.name.clone(),
                    round: req.round,
                    fetched_at: now,
                })
                .collect();
            Ok(out)
        }
    }

    struct Harness {
        _dir: TempDir,
        sink: Arc<FindingsSink>,
        db: PathBuf,
    }

    fn harness() -> Harness {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("omni.db");
        let conn = rusqlite::Connection::open(&db).expect("db acilir");
        conn.execute_batch(SCHEMA).expect("sema kurulur");
        conn.execute(
            "INSERT INTO tasks (id, root_id, title, mode, status) VALUES (1, 1, 't', 'mvp', 'open')",
            [],
        )
        .expect("gorev eklenir");
        drop(conn);

        let sink =
            Arc::new(FindingsSink::open(&db, &dir.path().join("cas")).expect("sink acilir"));
        Harness {
            _dir: dir,
            sink,
            db,
        }
    }

    fn rows(db: &Path) -> Vec<(String, String, String, String)> {
        let conn = rusqlite::Connection::open(db).expect("db acilir");
        let mut stmt = conn
            .prepare("SELECT mode, format, result_ref, query FROM research_findings ORDER BY id")
            .expect("sorgu");
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .expect("satirlar");
        rows.filter_map(Result::ok).collect()
    }

    #[tokio::test]
    async fn uc_modda_sonuc_research_findings_e_yazilir() {
        let h = harness();
        let engine = ResearchEngine::new(
            Arc::new(FakeProvider::new("sahte", 3)),
            Arc::clone(&h.sink),
        );

        for mode in ResearchMode::ALL {
            let out = engine
                .investigate(1, "tokio aktor modeli", mode)
                .await
                .expect("arastirma calisir");
            assert_eq!(out.report.mode, mode);
            assert!(!out.report.findings.is_empty());
            assert_eq!(out.json.format, ResearchFormat::Json);
            assert_eq!(out.markdown.format, ResearchFormat::Markdown);
        }

        h.sink.writer.flush().await;
        let rows = rows(&h.db);
        assert_eq!(rows.len(), 6, "3 mod x 2 bicim");

        for mode in ResearchMode::ALL {
            let db_mode = mode.as_db_str();
            assert!(
                rows.iter()
                    .any(|(m, f, _, _)| m == db_mode && f == "json"),
                "{db_mode} icin json satiri yok"
            );
            assert!(
                rows.iter().any(|(m, f, _, _)| m == db_mode && f == "md"),
                "{db_mode} icin md satiri yok"
            );
        }
    }

    #[tokio::test]
    async fn govde_cas_te_result_ref_tabloda() {
        let h = harness();
        let engine = ResearchEngine::new(
            Arc::new(FakeProvider::new("sahte", 2)),
            Arc::clone(&h.sink),
        );
        let out = engine
            .investigate(1, "wal replay", ResearchMode::Surface)
            .await
            .expect("arastirma calisir");

        let json_body = h
            .sink
            .load_body(&out.json.result_ref)
            .await
            .expect("cas okunur")
            .expect("govde var");
        let parsed: ResearchReport = serde_json::from_slice(&json_body).expect("json kanonik");
        assert_eq!(parsed.query, "wal replay");
        assert_eq!(parsed.findings.len(), out.report.findings.len());

        let md_body = h
            .sink
            .load_body(&out.markdown.result_ref)
            .await
            .expect("cas okunur")
            .expect("govde var");
        let md = String::from_utf8(md_body).expect("utf8");
        assert!(md.starts_with("# Arastirma: wal replay"));
        assert!(md.contains("## Bulgular"));
    }

    #[tokio::test]
    async fn niyet_kaydi_yan_etkiden_once_ve_applied_1_ile_kapanir() {
        let h = harness();
        let engine = ResearchEngine::new(
            Arc::new(FakeProvider::new("sahte", 1)),
            Arc::clone(&h.sink),
        );
        let out = engine
            .investigate(1, "i7 niyet", ResearchMode::Surface)
            .await
            .expect("arastirma calisir");
        h.sink.writer.flush().await;

        let conn = rusqlite::Connection::open(&h.db).expect("db acilir");
        for op_id in [&out.json.op_id, &out.markdown.op_id] {
            let applied: i64 = conn
                .query_row(
                    "SELECT applied FROM write_journal WHERE op_id = ?1",
                    rusqlite::params![op_id],
                    |r| r.get(0),
                )
                .expect("niyet kaydi bulunur");
            assert_eq!(applied, 1, "niyet kapanmis olmali");
        }
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM write_journal WHERE applied = 0",
                [],
                |r| r.get(0),
            )
            .expect("sayim");
        assert_eq!(pending, 0);
    }

    /// Kapinin ikinci yarisi: saglayici degisince cekirdek degismez.
    ///
    /// Ayni [`ResearchEngine`] tipi, ayni `investigate` cagrisi, iki farkli
    /// `ResearchProvider` uygulamasi. Cekirdek kodda tek satir fark yok.
    #[tokio::test]
    async fn saglayici_degisince_cekirdek_degismez() {
        let h = harness();

        let birinci = ResearchEngine::new(
            Arc::new(FakeProvider::new("firecrawl-benzeri", 3)),
            Arc::clone(&h.sink),
        );
        let ikinci = ResearchEngine::new(
            Arc::new(FakeProvider::new("native-crawler", 3)),
            Arc::clone(&h.sink),
        );

        let a = birinci
            .investigate(1, "saglayici degisimi", ResearchMode::Deep)
            .await
            .expect("birinci saglayici");
        let b = ikinci
            .investigate(1, "saglayici degisimi", ResearchMode::Deep)
            .await
            .expect("ikinci saglayici");

        assert_eq!(a.report.provider, "firecrawl-benzeri");
        assert_eq!(b.report.provider, "native-crawler");
        // Ayni mod, ayni butce, ayni dongu: sorgu sayisi birebir ayni.
        assert_eq!(a.report.queries.len(), b.report.queries.len());
        assert_eq!(a.report.rounds_run, b.report.rounds_run);
        assert_eq!(a.report.findings.len(), b.report.findings.len());

        h.sink.writer.flush().await;
        let rows = rows(&h.db);
        assert_eq!(rows.len(), 4, "2 saglayici x 2 bicim");
    }

    /// Config'ten saglayici uretimi cekirdegi degistirmez: `ResearchEngine`
    /// yine ayni tiptir, yalnizca `dyn` arkasindaki uygulama farklidir.
    #[test]
    fn config_saglayiciyi_secer() {
        let raw = serde_json::json!({
            "kind": "mcp",
            "server_name": "exa",
            "url": "https://ornek.invalid/mcp",
            "tool": "web_search_exa"
        });
        let cfg: ProviderConfig = serde_json::from_value(raw).expect("config cozulur");
        let provider = cfg.build().expect("saglayici kurulur");
        assert_eq!(provider.name(), "exa");
    }

    #[tokio::test]
    async fn butce_asilmaz() {
        let h = harness();
        // Saglayici mod butcesinden fazlasini donse bile cekirdek keser.
        let engine = ResearchEngine::new(
            Arc::new(FakeProvider::new("bol", 500)),
            Arc::clone(&h.sink),
        );
        let report = engine
            .collect(1, "butce", ResearchMode::Surface)
            .await
            .expect("toplama calisir");
        let params = ResearchMode::Surface.params();
        assert!(report.findings.len() <= params.max_sources);
        assert!(report.queries.len() <= params.max_queries);
        assert!(report.truncated);
    }

    #[tokio::test]
    async fn bos_sorgu_reddedilir() {
        let h = harness();
        let engine = ResearchEngine::new(
            Arc::new(FakeProvider::new("sahte", 1)),
            Arc::clone(&h.sink),
        );
        let err = engine
            .collect(1, "   ", ResearchMode::Deep)
            .await
            .expect_err("bos sorgu reddedilmeli");
        assert!(matches!(err, ResearchError::Invalid(_)));
    }

    #[test]
    fn genisletme_deterministik() {
        let now = omni_proto::now();
        let findings: Vec<Finding> = (0..3)
            .map(|i| Finding {
                url: format!("https://x.invalid/{i}"),
                title: "tokio kanal aktor modeli".into(),
                snippet: "kanal aktor".into(),
                content: None,
                score: 1.0,
                source: "s".into(),
                round: 0,
                fetched_at: now,
            })
            .collect();

        let a = refine_queries("tokio", &findings, 2, &[]);
        let b = refine_queries("tokio", &findings, 2, &[]);
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|q| q.starts_with("tokio ")));
        // Kok sorgudaki sozcuk tekrar eklenmez.
        assert!(!a.iter().any(|q| q == "tokio tokio"));
    }

    #[test]
    fn markdown_json_den_turer() {
        let now = omni_proto::now();
        let report = ResearchReport {
            task_id: 7,
            mode: ResearchMode::Ocean,
            query: "kanonik".into(),
            provider: "sahte".into(),
            generated_at: now,
            rounds_run: 2,
            queries: vec!["kanonik".into()],
            findings: vec![Finding {
                url: "https://a.invalid".into(),
                title: "Baslik\nikinci satir".into(),
                snippet: "ozet".into(),
                content: Some("govde".into()),
                score: 0.5,
                source: "sahte".into(),
                round: 0,
                fetched_at: now,
            }],
            truncated: false,
        };

        let json = report.to_json_bytes().expect("json");
        let geri: ResearchReport = serde_json::from_slice(&json).expect("gidis donus");
        assert_eq!(geri.findings.len(), 1);

        let md = report.to_markdown();
        assert!(md.contains("`ocean`"));
        assert!(md.contains("<https://a.invalid>"));
        // Satir sonu markdown basligini bozmamali.
        assert!(md.contains("### 1. Baslik ikinci satir"));
    }

    #[test]
    fn bicim_db_gidis_donus() {
        for f in [ResearchFormat::Json, ResearchFormat::Markdown] {
            assert_eq!(ResearchFormat::from_db_str(f.as_db_str()), Some(f));
        }
        assert_eq!(ResearchFormat::from_db_str("csv"), None);
    }

    #[test]
    fn url_tekilleme_normalize_eder() {
        let now = omni_proto::now();
        let mk = |url: &str| Finding {
            url: url.into(),
            title: "t".into(),
            snippet: String::new(),
            content: None,
            score: 0.0,
            source: "s".into(),
            round: 0,
            fetched_at: now,
        };
        assert_eq!(
            mk("https://www.A.invalid/x/").dedup_key(),
            mk("http://a.invalid/x").dedup_key()
        );
    }
}
