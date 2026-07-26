//! Tetiklemeli medya yakalama + `recordings` deposu (MASTER-PLAN 17.1, AS6).
//!
//! Event-log her zaman aciktir; **video/ekran/DOM ise yalniz computer-use
//! oturumu acikken** kaydedilir. Bu modul o kapinin bekcisidir:
//!
//!   1. Kayit istegi bir [`RecordingTrigger`] ile gelir. Oturumun acik olup
//!      olmadigina **cagiran** karar verir (computer-use hub'i, 17.3); burada
//!      yalnizca durum okunur. Tetik kapaliysa istek [`MediaError::Inactive`]
//!      ile reddedilir ve tek bayt bile yazilmaz.
//!   2. Govde zstd ile sikistirilip CAS'a yazilir, satir `recordings`
//!      tablosuna duser (migrations/0006: `agent_id`, `media_type`, `blob_ref`,
//!      `bytes`, `codec`, `started_at`, `ended_at`).
//!   3. Blob deftere **TTL ile** girer ([`BlobKind::Media`]) ve satir onun
//!      sahibi olur ([`RefOwner::recording`]). TTL dolunca [`MediaStore::sweep`]
//!      satiri siler, atifi dusurur ve refcount GC'si blob'u toplar (14.3).
//!
//! Yakalama arka ucu soyutlanmistir (K6): Wayland ve X11 ayni
//! [`CaptureBackend`] arkasindadir. Gercek kare uretimi computer-use hub'inin
//! isidir; bu crate kareyi alir, sikistirir, kalici hale getirir.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use omni_storage::cas::CasBlobStore;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;

use crate::compressor::ZstdCompressor;
use crate::db::{open_conn, parse_ts, placeholders, ts_text, tune};
use crate::error::RecordError;
use crate::refcount::{BlobKind, CasRefcounts, GcOutcome, RefOwner};

/// Medya blob'larinin varsayilan yasam suresi (17.1 "medya TTL'li").
pub const DEFAULT_MEDIA_TTL_DAYS: i64 = 7;

/// Tek `IN (...)` sorgusuna konan en fazla yer tutucu (SQLite degisken siniri).
const MAX_IN_PARAMS: usize = 256;

/// `recordings.media_type` degerleri (17.1: video / ekran / DOM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MediaKind {
    /// Zaman icinde biriken kare dizisi (bkz. [`crate::video`]).
    Video,
    /// Tek ekran goruntusu.
    Screen,
    /// Tarayici DOM anlik goruntusu.
    Dom,
}

impl MediaKind {
    /// Tabloya yazilan metin.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Screen => "screen",
            Self::Dom => "dom",
        }
    }

    /// Tablodan okunan metni cozer.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "video" => Some(Self::Video),
            "screen" => Some(Self::Screen),
            "dom" => Some(Self::Dom),
            _ => None,
        }
    }
}

/// Acik bir computer-use oturumunun kimligi.
///
/// `agent_id` dogrudan `recordings.agent_id` sutununa yazilir; `session_id`
/// yalniz izleme/gunluk icindir (0006 semasinda karsilik sutun yok).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerUseSession {
    /// Oturumu acan ajan (`agents.id`).
    pub agent_id: i64,
    /// Hub tarafindan uretilen oturum kimligi.
    pub session_id: String,
}

impl ComputerUseSession {
    /// Yeni oturum tanimlayicisi.
    pub fn new(agent_id: i64, session_id: impl Into<String>) -> Self {
        Self {
            agent_id,
            session_id: session_id.into(),
        }
    }
}

/// Tetigin o andaki durumu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerState {
    /// Computer-use oturumu acik; medya kaydi serbest.
    Active(ComputerUseSession),
    /// Oturum kapali; medya kaydi yasak.
    Idle,
}

impl TriggerState {
    /// Kayit serbest mi?
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active(_))
    }

    /// Acik oturumu tuketerek dondurur.
    #[must_use]
    pub fn into_session(self) -> Option<ComputerUseSession> {
        match self {
            Self::Active(s) => Some(s),
            Self::Idle => None,
        }
    }
}

/// Medya kaydini tetikleyen kaynak.
///
/// Bu crate **karar vermez**: computer-use oturumunun acik olup olmadigini
/// bilen taraf (hub / zamanlayici) bu trait'i uygular, kayit yollari yalnizca
/// [`RecordingTrigger::trigger_state`] okur.
pub trait RecordingTrigger: Send + Sync {
    /// Tetigin o andaki durumu.
    fn trigger_state(&self) -> TriggerState;
}

impl<T: RecordingTrigger + ?Sized> RecordingTrigger for Arc<T> {
    fn trigger_state(&self) -> TriggerState {
        (**self).trigger_state()
    }
}

impl<T: RecordingTrigger + ?Sized> RecordingTrigger for &T {
    fn trigger_state(&self) -> TriggerState {
        (**self).trigger_state()
    }
}

/// Hub'in acip kapadigi paylasilan tetik.
///
/// Klonlari ayni durumu gorur: oturum acildiginda kayit yollari calisir,
/// kapandiginda ayni an durur.
#[derive(Debug, Clone, Default)]
pub struct SharedTrigger {
    inner: Arc<Mutex<Option<ComputerUseSession>>>,
}

impl SharedTrigger {
    /// Kapali tetik.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acik oturumla baslatilmis tetik.
    #[must_use]
    pub fn opened(session: ComputerUseSession) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(session))),
        }
    }

    /// Computer-use oturumunu acar.
    pub fn open(&self, session: ComputerUseSession) {
        tracing::info!(
            agent_id = session.agent_id,
            session = %session.session_id,
            "medya kayit tetigi acildi"
        );
        *self.inner.lock() = Some(session);
    }

    /// Oturumu kapatir; sonraki kayit istekleri reddedilir.
    pub fn close(&self) {
        if let Some(prev) = self.inner.lock().take() {
            tracing::info!(
                agent_id = prev.agent_id,
                session = %prev.session_id,
                "medya kayit tetigi kapandi"
            );
        }
    }
}

impl RecordingTrigger for SharedTrigger {
    fn trigger_state(&self) -> TriggerState {
        match self.inner.lock().as_ref() {
            Some(s) => TriggerState::Active(s.clone()),
            None => TriggerState::Idle,
        }
    }
}

/// Medya yollarinin hata tipi.
///
/// `error.rs` tetik reddini tanimaz; kapi hatasi burada yasar, geri kalan her
/// sey [`RecordError`]'e devredilir (I6: uretim yolunda panik yok).
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// Computer-use oturumu kapali; 17.1 geregi kayit baslamaz.
    #[error("computer-use oturumu acik degil; medya kaydi tetiklenmedi")]
    Inactive,

    /// Yakalama arka ucu bu ortamda kullanilamiyor (K6).
    #[error("yakalama arka ucu kullanilamiyor: {0}")]
    BackendUnavailable(String),

    /// Kayit katmani hatasi (CAS / SQLite / sikistirma).
    #[error(transparent)]
    Record(#[from] RecordError),
}

impl From<crate::compressor::CompressError> for MediaError {
    fn from(e: crate::compressor::CompressError) -> Self {
        Self::Record(RecordError::Compress(e))
    }
}

impl From<rusqlite::Error> for MediaError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Record(RecordError::from(e))
    }
}

impl From<omni_storage::traits::StorageError> for MediaError {
    fn from(e: omni_storage::traits::StorageError) -> Self {
        Self::Record(RecordError::Storage(e))
    }
}

/// Tetik acikken oturumu dondurur, kapaliysa reddeder.
///
/// # Errors
/// Tetik kapaliysa [`MediaError::Inactive`] doner.
pub fn require_session(trigger: &dyn RecordingTrigger) -> Result<ComputerUseSession, MediaError> {
    trigger
        .trigger_state()
        .into_session()
        .ok_or(MediaError::Inactive)
}

/// Masaustu oturum tipi (K6: Linux'ta Wayland **ve** X11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DisplayServer {
    /// Wayland bilesikleyicisi.
    Wayland,
    /// X11 sunucusu.
    X11,
    /// Grafik oturumu yok (bassiz calisma).
    Headless,
}

impl DisplayServer {
    /// Kisa ad; gunluk ve `codec` etiketlerinde kullanilir.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::X11 => "x11",
            Self::Headless => "headless",
        }
    }
}

/// Ortamdan masaustu oturum tipini cikarir.
///
/// Wayland ve X11 ayni anda tanimli olabilir (XWayland); bu durumda yerel
/// bilesikleyici tercih edilir.
#[must_use]
pub fn detect_display_server() -> DisplayServer {
    let tanimli = |k: &str| std::env::var(k).is_ok_and(|v| !v.trim().is_empty());
    if tanimli("WAYLAND_DISPLAY") {
        DisplayServer::Wayland
    } else if tanimli("DISPLAY") {
        DisplayServer::X11
    } else {
        DisplayServer::Headless
    }
}

/// Kare ureten arka uc (K6 soyutlamasi).
///
/// Gercek Wayland/X11 yakalayicisi computer-use hub'inda yasar (17.3); kayit
/// yolu onu yalnizca bu trait uzerinden gorur, boylece arka uctan bagimsizdir.
pub trait CaptureBackend: Send + Sync {
    /// Arka ucun bagli oldugu masaustu oturumu.
    fn display_server(&self) -> DisplayServer;

    /// Uretilen karelerin ham bicimi (orn. `png`, `raw-rgba`).
    fn frame_codec(&self) -> &str;

    /// Tek kare uretir.
    ///
    /// # Errors
    /// Arka uc kare uretemezse [`MediaError`] doner.
    fn grab(&self) -> Result<Vec<u8>, MediaError>;
}

/// Onceden beslenmis kareleri sirayla veren arka uc.
///
/// Hub kareyi kendi uretir ve buraya birakir; testler de ayni yolu kullanir.
#[derive(Debug)]
pub struct BufferBackend {
    server: DisplayServer,
    codec: String,
    frames: Mutex<VecDeque<Vec<u8>>>,
}

impl BufferBackend {
    /// Bos tampon.
    pub fn new(server: DisplayServer, codec: impl Into<String>) -> Self {
        Self {
            server,
            codec: codec.into(),
            frames: Mutex::new(VecDeque::new()),
        }
    }

    /// Arka uca bir kare birakir.
    pub fn feed(&self, frame: Vec<u8>) {
        self.frames.lock().push_back(frame);
    }

    /// Bekleyen kare sayisi.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.frames.lock().len()
    }
}

impl CaptureBackend for BufferBackend {
    fn display_server(&self) -> DisplayServer {
        self.server
    }

    fn frame_codec(&self) -> &str {
        &self.codec
    }

    fn grab(&self) -> Result<Vec<u8>, MediaError> {
        self.frames
            .lock()
            .pop_front()
            .ok_or_else(|| MediaError::BackendUnavailable("bekleyen kare yok".to_string()))
    }
}

/// Bu ortamda yakalama yapilamadigini bildiren arka uc.
///
/// Bassiz calisirken (`DISPLAY`/`WAYLAND_DISPLAY` yok) kayit yolunun panik
/// yerine duzgun hata dondurmesini saglar (I6).
#[derive(Debug, Clone)]
pub struct UnavailableBackend {
    server: DisplayServer,
    reason: String,
}

impl UnavailableBackend {
    /// Gerekce ile kurar.
    pub fn new(server: DisplayServer, reason: impl Into<String>) -> Self {
        Self {
            server,
            reason: reason.into(),
        }
    }
}

impl CaptureBackend for UnavailableBackend {
    fn display_server(&self) -> DisplayServer {
        self.server
    }

    fn frame_codec(&self) -> &str {
        "none"
    }

    fn grab(&self) -> Result<Vec<u8>, MediaError> {
        Err(MediaError::BackendUnavailable(self.reason.clone()))
    }
}

/// `recordings` tablosuna yazilacak tek medya parcasi.
#[derive(Debug, Clone)]
pub struct MediaWrite<'a> {
    /// Kaydi tetikleyen acik oturum.
    pub session: &'a ComputerUseSession,
    /// Medya sinifi (`media_type`).
    pub kind: MediaKind,
    /// Ham govde; CAS'a zstd ile sikistirilarak yazilir.
    pub body: &'a [u8],
    /// Ham govdenin bicimi (`codec`); bilinmiyorsa `None`.
    pub codec: Option<&'a str>,
    /// Parcanin baslangici (`started_at`).
    pub started_at: DateTime<Utc>,
    /// Parcanin bitisi (`ended_at`); akis surerken `None`.
    pub ended_at: Option<DateTime<Utc>>,
}

/// Kalici hale getirilmis medya parcasinin makbuzu.
#[derive(Debug, Clone, Serialize)]
pub struct StoredMedia {
    /// `recordings.id`.
    pub recording_id: i64,
    /// CAS icerik-hash'i (`blob_ref`).
    pub blob_ref: String,
    /// `media_type`.
    pub media_type: String,
    /// Ham bayt sayisi (`bytes`).
    pub bytes: u64,
    /// CAS'a yazilan sikistirilmis bayt sayisi.
    pub compressed_bytes: u64,
    /// `codec`.
    pub codec: Option<String>,
    /// TTL sonu; bu andan sonra [`MediaStore::sweep`] toplar.
    pub expires_at: DateTime<Utc>,
}

/// `recordings` tablosundan okunan satir.
#[derive(Debug, Clone)]
pub struct RecordingRow {
    /// Satir kimligi.
    pub id: i64,
    /// Kaydi tutan ajan.
    pub agent_id: i64,
    /// `media_type`.
    pub media_type: String,
    /// CAS icerik-hash'i.
    pub blob_ref: String,
    /// Ham bayt sayisi; sutun bos ise `None`.
    pub bytes: Option<i64>,
    /// Ham govde bicimi.
    pub codec: Option<String>,
    /// Baslangic.
    pub started_at: Option<DateTime<Utc>>,
    /// Bitis; akis surerken `None`.
    pub ended_at: Option<DateTime<Utc>>,
}

/// TTL suresi dolan medyanin toplanma raporu.
#[derive(Debug, Clone, Default)]
pub struct MediaSweep {
    /// Suresi dolmus blob hash'leri.
    pub expired_blobs: Vec<String>,
    /// Silinen `recordings` satir sayisi.
    pub deleted_rows: usize,
    /// Refcount GC sonucu.
    pub gc: GcOutcome,
}

/// Tetiklemeli medyanin kalici deposu: CAS + `recordings` + TTL'li defter.
pub struct MediaStore {
    conn: Mutex<Connection>,
    refs: Arc<CasRefcounts>,
    ttl: Duration,
}

impl MediaStore {
    /// Veritabani yolundan acar; defter cagiranla paylasilir (event-log ile
    /// ayni defter, 14.3).
    ///
    /// # Errors
    /// Baglanti acilamazsa [`RecordError`] doner.
    pub fn open(db_path: &Path, refs: Arc<CasRefcounts>) -> Result<Self, RecordError> {
        let conn = open_conn(db_path)?;
        Self::with_connection(conn, refs)
    }

    /// Var olan baglanti uzerine kurar (test/bellek-ici kullanim).
    ///
    /// # Errors
    /// Pragma'lar uygulanamazsa [`RecordError`] doner.
    pub fn with_connection(conn: Connection, refs: Arc<CasRefcounts>) -> Result<Self, RecordError> {
        tune(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            refs,
            ttl: Duration::days(DEFAULT_MEDIA_TTL_DAYS),
        })
    }

    /// Medya TTL'ini degistirir (17.1 retention ayari).
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Etkin TTL.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Paylasilan atif defteri.
    #[must_use]
    pub fn refcounts(&self) -> &Arc<CasRefcounts> {
        &self.refs
    }

    /// Blob deposu.
    #[must_use]
    pub fn cas(&self) -> &CasBlobStore {
        self.refs.cas()
    }

    /// Medya parcasini kalici hale getirir: sikistir -> CAS -> `recordings`
    /// satiri -> TTL'li defter kaydi + satir atifi.
    ///
    /// Tetik dogrulamasini cagiran yapmis olmalidir; kapiyi ayni cagride
    /// gecmek icin [`MediaStore::store_triggered`] kullanilir.
    ///
    /// # Errors
    /// Sikistirma, CAS yazimi ya da SQLite yazimi basarisiz olursa
    /// [`MediaError`] doner.
    pub fn store(&self, write: &MediaWrite<'_>) -> Result<StoredMedia, MediaError> {
        let raw_bytes = write.body.len() as u64;
        let compressed = ZstdCompressor::compress(write.body)?;
        let compressed_bytes = compressed.len() as u64;
        let blob_ref = self.refs.cas().store(&compressed, false)?;

        // Once defter: blob TTL ile izlenmeye baslar, satir sonra sahiplenir.
        // Ters sirada cokme olursa deftersiz (dolayisiyla GC gormeyen) blob
        // kalirdi.
        let now = Utc::now();
        let expires_at = now.checked_add_signed(self.ttl).unwrap_or(now);
        self.refs
            .track(&blob_ref, BlobKind::Media, raw_bytes, Some(self.ttl))?;

        let recording_id = self.insert_row(write, &blob_ref, raw_bytes)?;
        self.refs
            .retain(&blob_ref, &RefOwner::recording(recording_id))?;

        tracing::info!(
            agent_id = write.session.agent_id,
            session = %write.session.session_id,
            media_type = write.kind.as_db_str(),
            recording_id,
            blob = %blob_ref,
            bytes = raw_bytes,
            compressed_bytes,
            "tetiklemeli medya kaydedildi"
        );

        Ok(StoredMedia {
            recording_id,
            blob_ref,
            media_type: write.kind.as_db_str().to_string(),
            bytes: raw_bytes,
            compressed_bytes,
            codec: write.codec.map(str::to_string),
            expires_at,
        })
    }

    /// Tetigi dogrular ve yalniz acikken parcayi yazar.
    ///
    /// # Errors
    /// Tetik kapaliysa [`MediaError::Inactive`], yazim hatasinda ilgili
    /// [`MediaError`] doner.
    pub fn store_triggered(
        &self,
        trigger: &dyn RecordingTrigger,
        kind: MediaKind,
        body: &[u8],
        codec: Option<&str>,
    ) -> Result<StoredMedia, MediaError> {
        let session = require_session(trigger)?;
        let now = Utc::now();
        self.store(&MediaWrite {
            session: &session,
            kind,
            body,
            codec,
            started_at: now,
            ended_at: Some(now),
        })
    }

    /// Acik birakilmis bir satirin bitisini isaretler.
    ///
    /// # Errors
    /// Guncelleme basarisiz olursa [`MediaError`] doner.
    pub fn finish_row(&self, recording_id: i64, ended_at: DateTime<Utc>) -> Result<(), MediaError> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE recordings SET ended_at = ?1 WHERE id = ?2",
            rusqlite::params![ts_text(ended_at), recording_id],
        )?;
        Ok(())
    }

    /// Bir ajanin kayitlarini eskiden yeniye listeler.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`MediaError`] doner.
    pub fn recordings_of(&self, agent_id: i64) -> Result<Vec<RecordingRow>, MediaError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, media_type, blob_ref, bytes, codec, started_at, ended_at
             FROM recordings WHERE agent_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(rusqlite::params![agent_id], row_to_recording)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// TTL'i dolan medyayi toplar (17.1 retention): satirlari siler ->
    /// atiflari birakir -> refcount GC'si blob'u siler.
    ///
    /// Event-log blob'larina dokunulmaz; onlarin TTL'i yoktur.
    ///
    /// # Errors
    /// Defter sorgusu ya da satir silme basarisiz olursa [`MediaError`] doner.
    pub fn sweep(&self, now: DateTime<Utc>) -> Result<MediaSweep, MediaError> {
        let expired = self.refs.expired_media(now)?;
        if expired.is_empty() {
            return Ok(MediaSweep::default());
        }

        let mut deleted_rows = 0usize;
        for chunk in expired.chunks(MAX_IN_PARAMS) {
            for (id, blob_ref) in self.rows_for_blobs(chunk)? {
                // Once atif dusurulur, sonra satir silinir: ters sirada cokme
                // olursa defterde sahipsiz atif kalir ve blob hic toplanmazdi.
                self.refs.release(&blob_ref, &RefOwner::recording(id))?;
                let conn = self.conn.lock();
                deleted_rows +=
                    conn.execute("DELETE FROM recordings WHERE id = ?1", rusqlite::params![id])?;
            }
        }

        // Satir disinda kalan atiflar (varsa) da birakilir, ardindan GC.
        self.refs.expire_media(now)?;
        let gc = self.refs.collect()?;

        tracing::info!(
            expired = expired.len(),
            deleted_rows,
            deleted_blobs = gc.deleted.len(),
            "TTL'i dolan medya toplandi"
        );

        Ok(MediaSweep {
            expired_blobs: expired,
            deleted_rows,
            gc,
        })
    }

    /// Verilen blob'lara atif tutan `recordings` satirlarini bulur.
    fn rows_for_blobs(&self, blobs: &[String]) -> Result<Vec<(i64, String)>, MediaError> {
        if blobs.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT id, blob_ref FROM recordings WHERE blob_ref IN ({})",
            placeholders(blobs.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(blobs.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// `recordings` satirini yazar ve `rowid`'sini dondurur.
    fn insert_row(
        &self,
        write: &MediaWrite<'_>,
        blob_ref: &str,
        raw_bytes: u64,
    ) -> Result<i64, MediaError> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO recordings
                 (agent_id, media_type, blob_ref, bytes, codec, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                write.session.agent_id,
                write.kind.as_db_str(),
                blob_ref,
                i64::try_from(raw_bytes).unwrap_or(i64::MAX),
                write.codec,
                ts_text(write.started_at),
                write.ended_at.map(ts_text),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

/// Sorgu satirini [`RecordingRow`]'a cevirir.
fn row_to_recording(row: &rusqlite::Row<'_>) -> Result<RecordingRow, rusqlite::Error> {
    let started: String = row.get(6)?;
    let ended: Option<String> = row.get(7)?;
    Ok(RecordingRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        media_type: row.get(2)?,
        blob_ref: row.get(3)?,
        bytes: row.get(4)?,
        codec: row.get(5)?,
        started_at: parse_ts(&started),
        ended_at: ended.as_deref().and_then(parse_ts),
    })
}

/// Tetiklemeli ekran/DOM yakalayicisi.
///
/// Arka uc (Wayland/X11/bassiz) disaridan verilir; bu tip yalnizca tetigi
/// dogrular, kareyi alir ve [`MediaStore`]'a birakir.
pub struct ScreenCapture<B: CaptureBackend> {
    backend: B,
}

impl<B: CaptureBackend> ScreenCapture<B> {
    /// Verilen arka uc ile kurar.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Arka ucun masaustu oturumu (K6).
    pub fn display_server(&self) -> DisplayServer {
        self.backend.display_server()
    }

    /// Alttaki arka uc.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Tetik acikken tek kare alir; kapaliysa arka uca **dokunmaz**.
    ///
    /// # Errors
    /// Tetik kapaliysa [`MediaError::Inactive`], arka uc kare uretemezse
    /// [`MediaError::BackendUnavailable`] doner.
    pub fn grab(&self, trigger: &dyn RecordingTrigger) -> Result<Vec<u8>, MediaError> {
        require_session(trigger)?;
        self.backend.grab()
    }

    /// Tetik acikken kare alir ve `recordings` tablosuna kalici hale getirir.
    ///
    /// # Errors
    /// Tetik kapali, arka uc kullanilamaz ya da yazim basarisiz olursa
    /// [`MediaError`] doner.
    pub fn capture_to_store(
        &self,
        trigger: &dyn RecordingTrigger,
        store: &MediaStore,
        kind: MediaKind,
    ) -> Result<StoredMedia, MediaError> {
        let session = require_session(trigger)?;
        let started_at = Utc::now();
        let frame = self.backend.grab()?;
        store.store(&MediaWrite {
            session: &session,
            kind,
            body: &frame,
            codec: Some(self.backend.frame_codec()),
            started_at,
            ended_at: Some(Utc::now()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SEED_AGENT_ID, semali_db};

    fn oturum() -> ComputerUseSession {
        ComputerUseSession::new(SEED_AGENT_ID, "sess-1")
    }

    fn kurulum(dir: &Path) -> (MediaStore, Arc<CasRefcounts>) {
        let db_path = semali_db(dir);
        let cas = CasBlobStore::new(&dir.join("cas")).expect("cas acilmali");
        let refs = Arc::new(CasRefcounts::open(&db_path, cas).expect("defter acilmali"));
        let store = MediaStore::open(&db_path, Arc::clone(&refs)).expect("depo acilmali");
        (store, refs)
    }

    #[test]
    fn tetik_kapaliyken_kayit_reddedilir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, refs) = kurulum(dir.path());
        let trigger = SharedTrigger::new();

        let res = store.store_triggered(&trigger, MediaKind::Screen, b"kare", Some("png"));
        assert!(matches!(res, Err(MediaError::Inactive)));
        assert_eq!(refs.tracked_count().expect("defter sayimi"), 0);
    }

    #[test]
    fn tetik_acikken_satir_ve_atif_olusur() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, refs) = kurulum(dir.path());
        let trigger = SharedTrigger::opened(oturum());

        let stored = store
            .store_triggered(&trigger, MediaKind::Screen, b"kare govdesi", Some("png"))
            .expect("kayit yazilmali");

        assert_eq!(stored.media_type, "screen");
        assert_eq!(stored.bytes, "kare govdesi".len() as u64);
        assert_eq!(
            refs.refcount(&stored.blob_ref).expect("refcount okunmali"),
            1
        );

        let rows = store
            .recordings_of(SEED_AGENT_ID)
            .expect("satirlar okunmali");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].blob_ref, stored.blob_ref);
        assert_eq!(rows[0].codec.as_deref(), Some("png"));
        assert!(rows[0].started_at.is_some());
        assert!(rows[0].ended_at.is_some());
    }

    #[test]
    fn tetik_kapaliyken_arka_uc_cagrilmaz() {
        let backend = BufferBackend::new(DisplayServer::Wayland, "raw-rgba");
        backend.feed(vec![1, 2, 3]);
        let cap = ScreenCapture::new(backend);
        let trigger = SharedTrigger::new();

        assert!(matches!(cap.grab(&trigger), Err(MediaError::Inactive)));
        assert_eq!(cap.backend().pending(), 1, "kare tuketilmemeli");

        trigger.open(oturum());
        assert_eq!(cap.grab(&trigger).expect("kare gelmeli"), vec![1, 2, 3]);
        assert_eq!(cap.backend().pending(), 0);
    }

    #[test]
    fn her_iki_arka_uc_de_ayni_yoldan_kaydedilir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, _refs) = kurulum(dir.path());
        let trigger = SharedTrigger::opened(oturum());

        for server in [DisplayServer::Wayland, DisplayServer::X11] {
            let backend = BufferBackend::new(server, server.as_str());
            backend.feed(server.as_str().as_bytes().to_vec());
            let cap = ScreenCapture::new(backend);
            let stored = cap
                .capture_to_store(&trigger, &store, MediaKind::Screen)
                .expect("kayit yazilmali");
            assert_eq!(stored.codec.as_deref(), Some(server.as_str()));
        }

        let rows = store
            .recordings_of(SEED_AGENT_ID)
            .expect("satirlar okunmali");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn kullanilamayan_arka_uc_panik_yerine_hata_doner() {
        let cap = ScreenCapture::new(UnavailableBackend::new(
            DisplayServer::Headless,
            "grafik oturumu yok",
        ));
        let trigger = SharedTrigger::opened(oturum());
        assert!(matches!(
            cap.grab(&trigger),
            Err(MediaError::BackendUnavailable(_))
        ));
    }

    #[test]
    fn ttl_dolunca_satir_atif_ve_blob_toplanir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let db_path = semali_db(dir.path());
        let cas = CasBlobStore::new(&dir.path().join("cas")).expect("cas acilmali");
        let refs = Arc::new(CasRefcounts::open(&db_path, cas).expect("defter acilmali"));
        let store = MediaStore::open(&db_path, Arc::clone(&refs))
            .expect("depo acilmali")
            .with_ttl(Duration::seconds(1));
        let trigger = SharedTrigger::opened(oturum());

        let stored = store
            .store_triggered(&trigger, MediaKind::Dom, b"<html></html>", Some("html"))
            .expect("kayit yazilmali");

        // TTL dolmadan hicbir sey silinmez.
        let erken = store.sweep(Utc::now()).expect("sweep kosmali");
        assert!(erken.expired_blobs.is_empty());
        assert_eq!(refs.refcount(&stored.blob_ref).expect("refcount"), 1);

        let sonra = Utc::now() + Duration::seconds(120);
        let rapor = store.sweep(sonra).expect("sweep kosmali");
        assert_eq!(rapor.expired_blobs, vec![stored.blob_ref.clone()]);
        assert_eq!(rapor.deleted_rows, 1);
        assert!(rapor.gc.deleted.contains(&stored.blob_ref));
        assert!(
            store
                .recordings_of(SEED_AGENT_ID)
                .expect("satirlar")
                .is_empty()
        );
        assert!(
            store
                .cas()
                .load(&stored.blob_ref)
                .expect("cas okunmali")
                .is_none()
        );
    }

    #[test]
    fn medya_tipi_metni_gidip_gelir() {
        for kind in [MediaKind::Video, MediaKind::Screen, MediaKind::Dom] {
            assert_eq!(MediaKind::from_db_str(kind.as_db_str()), Some(kind));
        }
        assert_eq!(MediaKind::from_db_str("baska"), None);
    }
}
