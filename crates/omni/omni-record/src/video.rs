//! Tetiklemeli video kaydi (MASTER-PLAN 17.1, AS6).
//!
//! Kaydedici **kendi basina baslamaz**: [`VideoRecorder::start`] bir
//! [`RecordingTrigger`] ister ve yalnizca computer-use oturumu acikken calisir
//! (17.1). Tetik kapaliyken gelen kareler sessizce dusurulur; hicbir sey CAS'a
//! ya da `recordings` tablosuna yazilmaz.
//!
//! Kareler sinirli bir halka tamponda birikir (K2 RAM disiplini): tampon dolunca
//! en eski kare dusurulur, surec asla siniri asmaz. [`VideoRecorder::flush_segment`]
//! biriken kareleri tek bir **segment** govdesinde toplar; segment
//! [`crate::screen_cap::MediaStore`] uzerinden zstd ile sikistirilip CAS'a
//! yazilir, satir `recordings`'e duser ve blob TTL'li olarak deftere girer.
//! TTL dolunca `MediaStore::sweep` satiri siler, atif duser, GC blob'u toplar.
//!
//! Segment govdesi kendi kendine yeter (konteyner): baslik + kare basina
//! `pts`/uzunluk. Boylece replay tarafi tek blob'dan kare dizisini geri kurar.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use crate::error::RecordError;
use crate::screen_cap::{
    ComputerUseSession, MediaError, MediaKind, MediaStore, MediaWrite, RecordingTrigger, StoredMedia,
};

/// Halka tamponun varsayilan ust siniri (bayt).
pub const RING_BUFFER_CAPACITY: usize = 512 * 1024;

/// Segment konteynerinin imzasi.
const SEGMENT_MAGIC: &[u8; 8] = b"OMNIVID1";

/// Baslik: imza + kare sayisi.
const SEGMENT_HEADER_LEN: usize = SEGMENT_MAGIC.len() + 4;

/// Kare basi ust bilgi: `pts` (i64) + uzunluk (u32).
const FRAME_HEADER_LEN: usize = 12;

/// `recordings.codec` icin segment konteyner adi. Ham kare bicimi degil,
/// **konteyner** adidir; CAS govdesi ayrica zstd ile sarilir.
pub const VIDEO_SEGMENT_CODEC: &str = "omnivid1";

/// Tek video karesi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Ham kare govdesi (arka ucun urettigi bicimde).
    pub data: Vec<u8>,
    /// Sunum zaman damgasi (kaydedicinin sectigi olcekte).
    pub pts: i64,
}

/// Kare dizisini tek segment govdesine kodlar.
///
/// Bicim: `magic(8) | frame_count(u32) | [ pts(i64) | len(u32) | data ]*`
/// (hepsi little-endian).
///
/// # Errors
/// Kare sayisi ya da bir karenin boyutu `u32`'ye sigmazsa
/// [`RecordError::MalformedSegment`] doner.
pub fn encode_segment(frames: &[Frame]) -> Result<Vec<u8>, RecordError> {
    let count = u32::try_from(frames.len())
        .map_err(|_| RecordError::MalformedSegment("segmentteki kare sayisi u32'yi asiyor"))?;

    let total: usize = SEGMENT_HEADER_LEN
        + frames
            .iter()
            .map(|f| FRAME_HEADER_LEN + f.data.len())
            .sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(SEGMENT_MAGIC);
    out.extend_from_slice(&count.to_le_bytes());

    for frame in frames {
        let len = u32::try_from(frame.data.len())
            .map_err(|_| RecordError::MalformedSegment("kare govdesi u32'yi asiyor"))?;
        out.extend_from_slice(&frame.pts.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&frame.data);
    }
    Ok(out)
}

/// Segment govdesini kare dizisine cozer.
///
/// # Errors
/// Imza, kare sayisi ya da uzunluk alanlari tutarsizsa
/// [`RecordError::MalformedSegment`] doner (I6: panik yok).
pub fn decode_segment(body: &[u8]) -> Result<Vec<Frame>, RecordError> {
    if body.len() < SEGMENT_HEADER_LEN {
        return Err(RecordError::MalformedSegment("govde baslik icin cok kisa"));
    }
    if &body[..SEGMENT_MAGIC.len()] != SEGMENT_MAGIC {
        return Err(RecordError::MalformedSegment("segment imzasi uyusmuyor"));
    }

    let mut cursor = SEGMENT_MAGIC.len();
    let count = read_u32(body, &mut cursor)?;
    let mut frames = Vec::with_capacity(count as usize);

    for _ in 0..count {
        let pts = read_i64(body, &mut cursor)?;
        let len = read_u32(body, &mut cursor)? as usize;
        let end = cursor
            .checked_add(len)
            .ok_or(RecordError::MalformedSegment("kare uzunlugu tasiyor"))?;
        if end > body.len() {
            return Err(RecordError::MalformedSegment("kare govdesi eksik"));
        }
        frames.push(Frame {
            data: body[cursor..end].to_vec(),
            pts,
        });
        cursor = end;
    }

    if cursor != body.len() {
        return Err(RecordError::MalformedSegment("segment sonunda artik bayt"));
    }
    Ok(frames)
}

/// Little-endian `u32` okur ve imleci ilerletir.
fn read_u32(body: &[u8], cursor: &mut usize) -> Result<u32, RecordError> {
    let end = *cursor + 4;
    let slice = body
        .get(*cursor..end)
        .ok_or(RecordError::MalformedSegment("u32 alani eksik"))?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(slice);
    *cursor = end;
    Ok(u32::from_le_bytes(buf))
}

/// Little-endian `i64` okur ve imleci ilerletir.
fn read_i64(body: &[u8], cursor: &mut usize) -> Result<i64, RecordError> {
    let end = *cursor + 8;
    let slice = body
        .get(*cursor..end)
        .ok_or(RecordError::MalformedSegment("i64 alani eksik"))?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(slice);
    *cursor = end;
    Ok(i64::from_le_bytes(buf))
}

/// Kaydedicinin ic durumu; tetik kapaliyken `session` `None`'dir.
#[derive(Debug)]
struct RecorderState {
    session: Option<ComputerUseSession>,
    frames: VecDeque<Frame>,
    total_bytes: usize,
    capacity: usize,
    segment_started_at: Option<DateTime<Utc>>,
    dropped: u64,
}

impl RecorderState {
    fn new(capacity: usize) -> Self {
        Self {
            session: None,
            frames: VecDeque::new(),
            total_bytes: 0,
            capacity: capacity.max(1),
            segment_started_at: None,
            dropped: 0,
        }
    }

    /// Kareyi tampona alir; sinir asilirsa en eski kareler dusurulur.
    fn push(&mut self, frame: Frame) {
        self.total_bytes = self.total_bytes.saturating_add(frame.data.len());
        self.frames.push_back(frame);
        while self.total_bytes > self.capacity && self.frames.len() > 1 {
            if let Some(evicted) = self.frames.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.data.len());
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }

    fn drain(&mut self) -> Vec<Frame> {
        self.total_bytes = 0;
        self.frames.drain(..).collect()
    }
}

/// Tetiklemeli video kaydedicisi.
///
/// Kareleri kim uretirse uretsin (Wayland/X11 arka ucu, K6) kaydedici yalnizca
/// tetik acikken kabul eder ve segmentleri [`MediaStore`]'a birakir.
#[derive(Debug)]
pub struct VideoRecorder {
    state: Mutex<RecorderState>,
    codec: String,
}

impl VideoRecorder {
    /// Varsayilan tampon siniri ile kurar.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(RING_BUFFER_CAPACITY)
    }

    /// Tampon sinirini bayt cinsinden vererek kurar.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            state: Mutex::new(RecorderState::new(capacity)),
            codec: VIDEO_SEGMENT_CODEC.to_string(),
        }
    }

    /// `recordings.codec` degerini degistirir (konteyner adi).
    #[must_use]
    pub fn with_codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = codec.into();
        self
    }

    /// Kaydi **yalniz** computer-use oturumu acikken baslatir (17.1).
    ///
    /// Karar tetigin sahibinindir; burada durum okunur ve oturum kopyalanir.
    ///
    /// # Errors
    /// Tetik kapaliysa [`MediaError::Inactive`] doner.
    pub fn start(&self, trigger: &dyn RecordingTrigger) -> Result<ComputerUseSession, MediaError> {
        let session = crate::screen_cap::require_session(trigger)?;
        let mut state = self.state.lock();
        state.session = Some(session.clone());
        state.segment_started_at = Some(Utc::now());
        drop(state);
        tracing::info!(
            agent_id = session.agent_id,
            session = %session.session_id,
            "tetiklemeli video kaydi basladi"
        );
        Ok(session)
    }

    /// Kayit suruyor mu?
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state.lock().session.is_some()
    }

    /// Kaydi tetikleyen oturum (calismiyorsa `None`).
    #[must_use]
    pub fn session(&self) -> Option<ComputerUseSession> {
        self.state.lock().session.clone()
    }

    /// Tamponda bekleyen kare sayisi.
    #[must_use]
    pub fn buffered_frames(&self) -> usize {
        self.state.lock().frames.len()
    }

    /// Tamponda bekleyen ham bayt.
    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.state.lock().total_bytes
    }

    /// Tetik kapali oldugu ya da tampon tastigi icin dusurulen kare sayisi.
    #[must_use]
    pub fn dropped_frames(&self) -> u64 {
        self.state.lock().dropped
    }

    /// Kareyi tampona alir. Kayit calismiyorsa kare **kabul edilmez**.
    ///
    /// Donen deger karenin alinip alinmadigidir; cagiran taraf bunu akis
    /// hizini kismak icin kullanabilir.
    pub fn push_frame(&self, data: Vec<u8>, pts: i64) -> bool {
        let mut state = self.state.lock();
        if state.session.is_none() {
            state.dropped = state.dropped.saturating_add(1);
            drop(state);
            tracing::debug!("computer-use oturumu kapali; kare dusuruldu");
            return false;
        }
        state.push(Frame { data, pts });
        true
    }

    /// Tamponu bosaltir ve kareleri dondurur (kalici hale getirmeden).
    pub fn flush(&self) -> Vec<Frame> {
        let frames = self.state.lock().drain();
        tracing::debug!(count = frames.len(), "video tamponu bosaltildi");
        frames
    }

    /// Tamponu bosaltir, kareleri hicbir yere yazmadan atar.
    ///
    /// Oturum onay disi kapandiginda (kayit tutulmamali) kullanilir.
    pub fn discard(&self) {
        let mut state = self.state.lock();
        let atilan = state.frames.len() as u64;
        state.drain();
        state.dropped = state.dropped.saturating_add(atilan);
        state.segment_started_at = Some(Utc::now());
    }

    /// Biriken kareleri tek segment olarak CAS + `recordings`'e yazar.
    ///
    /// Tampon bossa yazim yapilmaz ve `Ok(None)` doner. Kayit calismiyorsa
    /// istek reddedilir.
    ///
    /// # Errors
    /// Kayit calismiyorsa [`MediaError::Inactive`]; kodlama, sikistirma ya da
    /// yazim basarisiz olursa ilgili [`MediaError`] doner.
    pub fn flush_segment(&self, store: &MediaStore) -> Result<Option<StoredMedia>, MediaError> {
        let (session, frames, started_at) = {
            let mut state = self.state.lock();
            let Some(session) = state.session.clone() else {
                return Err(MediaError::Inactive);
            };
            if state.frames.is_empty() {
                return Ok(None);
            }
            let started_at = state.segment_started_at.unwrap_or_else(Utc::now);
            let frames = state.drain();
            state.segment_started_at = Some(Utc::now());
            (session, frames, started_at)
        };

        self.persist(store, &session, &frames, started_at).map(Some)
    }

    /// Kaydi durdurur; kalan kareler son segment olarak yazilir.
    ///
    /// Kayit zaten durmussa `Ok(None)` doner (idempotent kapanis).
    ///
    /// # Errors
    /// Son segmentin yazimi basarisiz olursa [`MediaError`] doner.
    pub fn stop(&self, store: &MediaStore) -> Result<Option<StoredMedia>, MediaError> {
        let (session, frames, started_at) = {
            let mut state = self.state.lock();
            let Some(session) = state.session.take() else {
                return Ok(None);
            };
            let started_at = state.segment_started_at.take().unwrap_or_else(Utc::now);
            let frames = state.drain();
            (session, frames, started_at)
        };

        tracing::info!(
            agent_id = session.agent_id,
            session = %session.session_id,
            frames = frames.len(),
            "tetiklemeli video kaydi durduruldu"
        );

        if frames.is_empty() {
            return Ok(None);
        }
        self.persist(store, &session, &frames, started_at).map(Some)
    }

    /// Tetigi yeniden okur ve kaydin durumunu ona esitler.
    ///
    /// Oturum kapanmissa kalan kareler yazilip kayit durdurulur; oturum acik
    /// ama kayit durmussa yeniden baslatilir. Karar yine tetigindir.
    ///
    /// # Errors
    /// Kapanista son segment yazilamazsa [`MediaError`] doner.
    pub fn sync_with_trigger(
        &self,
        trigger: &dyn RecordingTrigger,
        store: &MediaStore,
    ) -> Result<Option<StoredMedia>, MediaError> {
        match trigger.trigger_state().into_session() {
            Some(session) => {
                let mut state = self.state.lock();
                if state.session.as_ref() != Some(&session) {
                    state.session = Some(session);
                    state.segment_started_at = Some(Utc::now());
                }
                Ok(None)
            }
            None => self.stop(store),
        }
    }

    /// Segmenti kodlar ve depoya birakir.
    fn persist(
        &self,
        store: &MediaStore,
        session: &ComputerUseSession,
        frames: &[Frame],
        started_at: DateTime<Utc>,
    ) -> Result<StoredMedia, MediaError> {
        let body = encode_segment(frames)?;
        let stored = store.store(&MediaWrite {
            session,
            kind: MediaKind::Video,
            body: &body,
            codec: Some(&self.codec),
            started_at,
            ended_at: Some(Utc::now()),
        })?;
        tracing::info!(
            recording_id = stored.recording_id,
            frames = frames.len(),
            bytes = stored.bytes,
            "video segmenti kaydedildi"
        );
        Ok(stored)
    }
}

impl Default for VideoRecorder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use chrono::Duration;
    use omni_storage::cas::CasBlobStore;

    use super::*;
    use crate::compressor::ZstdCompressor;
    use crate::refcount::CasRefcounts;
    use crate::screen_cap::SharedTrigger;
    use crate::test_support::{SEED_AGENT_ID, semali_db};

    fn oturum() -> ComputerUseSession {
        ComputerUseSession::new(SEED_AGENT_ID, "sess-video")
    }

    fn kurulum(dir: &Path) -> (MediaStore, Arc<CasRefcounts>) {
        let db_path = semali_db(dir);
        let cas = CasBlobStore::new(&dir.join("cas")).expect("cas acilmali");
        let refs = Arc::new(CasRefcounts::open(&db_path, cas).expect("defter acilmali"));
        let store = MediaStore::open(&db_path, Arc::clone(&refs)).expect("depo acilmali");
        (store, refs)
    }

    #[test]
    fn segment_kodlama_gidip_gelir() {
        let frames = vec![
            Frame {
                data: b"kare-1".to_vec(),
                pts: 0,
            },
            Frame {
                data: Vec::new(),
                pts: 33,
            },
            Frame {
                data: b"kare-3".to_vec(),
                pts: 66,
            },
        ];
        let body = encode_segment(&frames).expect("kodlanmali");
        assert_eq!(decode_segment(&body).expect("cozulmeli"), frames);
    }

    #[test]
    fn bozuk_segment_panik_yerine_hata_verir() {
        assert!(matches!(
            decode_segment(b"kisa"),
            Err(RecordError::MalformedSegment(_))
        ));
        assert!(matches!(
            decode_segment(&[0u8; 32]),
            Err(RecordError::MalformedSegment(_))
        ));

        let body = encode_segment(&[Frame {
            data: b"govde".to_vec(),
            pts: 1,
        }])
        .expect("kodlanmali");
        assert!(matches!(
            decode_segment(&body[..body.len() - 2]),
            Err(RecordError::MalformedSegment(_))
        ));
    }

    #[test]
    fn tetik_kapaliyken_baslamaz_ve_kare_kabul_etmez() {
        let rec = VideoRecorder::new();
        let trigger = SharedTrigger::new();

        assert!(matches!(rec.start(&trigger), Err(MediaError::Inactive)));
        assert!(!rec.is_running());
        assert!(!rec.push_frame(b"kare".to_vec(), 0));
        assert_eq!(rec.buffered_frames(), 0);
        assert_eq!(rec.dropped_frames(), 1);
    }

    #[test]
    fn kayit_calismazken_segment_yazilamaz() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, _refs) = kurulum(dir.path());
        let rec = VideoRecorder::new();
        assert!(matches!(
            rec.flush_segment(&store),
            Err(MediaError::Inactive)
        ));
        assert!(
            store
                .recordings_of(SEED_AGENT_ID)
                .expect("satirlar")
                .is_empty()
        );
    }

    #[test]
    fn tetik_acikken_segment_cas_ve_tabloya_duser() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, refs) = kurulum(dir.path());
        let trigger = SharedTrigger::opened(oturum());
        let rec = VideoRecorder::new();

        rec.start(&trigger).expect("kayit baslamali");
        assert!(rec.push_frame(b"kare-a".to_vec(), 0));
        assert!(rec.push_frame(b"kare-b".to_vec(), 40));

        let stored = rec
            .flush_segment(&store)
            .expect("segment yazilmali")
            .expect("segment bos olmamali");

        assert_eq!(stored.media_type, "video");
        assert_eq!(stored.codec.as_deref(), Some(VIDEO_SEGMENT_CODEC));
        assert_eq!(refs.refcount(&stored.blob_ref).expect("refcount"), 1);
        assert_eq!(rec.buffered_frames(), 0);

        let rows = store
            .recordings_of(SEED_AGENT_ID)
            .expect("satirlar");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].media_type, "video");
        assert!(rows[0].ended_at.is_some());

        // CAS govdesi zstd sarili segmenttir; kareler geri kurulabilmeli.
        let sarili = store
            .cas()
            .load(&stored.blob_ref)
            .expect("cas okunmali")
            .expect("blob durmali");
        let body = ZstdCompressor::decompress(&sarili).expect("acilmali");
        let frames = decode_segment(&body).expect("cozulmeli");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].pts, 40);
    }

    #[test]
    fn bos_tampon_satir_uretmez() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, _refs) = kurulum(dir.path());
        let trigger = SharedTrigger::opened(oturum());
        let rec = VideoRecorder::new();

        rec.start(&trigger).expect("kayit baslamali");
        assert!(rec.flush_segment(&store).expect("cagri gecmeli").is_none());
        assert!(rec.stop(&store).expect("durus gecmeli").is_none());
        assert!(!rec.is_running());
    }

    #[test]
    fn oturum_kapaninca_son_segment_yazilir_ve_kayit_durur() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, _refs) = kurulum(dir.path());
        let trigger = SharedTrigger::opened(oturum());
        let rec = VideoRecorder::new();

        rec.start(&trigger).expect("kayit baslamali");
        assert!(rec.push_frame(b"son-kare".to_vec(), 12));

        trigger.close();
        let stored = rec
            .sync_with_trigger(&trigger, &store)
            .expect("esitleme gecmeli")
            .expect("son segment yazilmali");

        assert_eq!(stored.media_type, "video");
        assert!(!rec.is_running());
        assert!(!rec.push_frame(b"gec-kare".to_vec(), 20));
        assert_eq!(
            store
                .recordings_of(SEED_AGENT_ID)
                .expect("satirlar")
                .len(),
            1
        );
    }

    #[test]
    fn tampon_sinirinda_en_eski_kare_dusurulur() {
        let rec = VideoRecorder::with_capacity(16);
        let trigger = SharedTrigger::opened(oturum());
        rec.start(&trigger).expect("kayit baslamali");

        for i in 0..8i64 {
            assert!(rec.push_frame(vec![b'x'; 8], i));
        }
        assert!(rec.buffered_bytes() <= 16);
        assert!(rec.dropped_frames() > 0);
    }

    #[test]
    fn atilan_kareler_hicbir_yere_yazilmaz() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let (store, refs) = kurulum(dir.path());
        let trigger = SharedTrigger::opened(oturum());
        let rec = VideoRecorder::new();

        rec.start(&trigger).expect("kayit baslamali");
        assert!(rec.push_frame(b"gizli".to_vec(), 0));
        rec.discard();

        assert_eq!(rec.buffered_frames(), 0);
        assert!(rec.flush_segment(&store).expect("cagri gecmeli").is_none());
        assert_eq!(refs.tracked_count().expect("defter sayimi"), 0);
    }

    #[test]
    fn ttl_dolunca_video_segmenti_toplanir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let db_path = semali_db(dir.path());
        let cas = CasBlobStore::new(&dir.path().join("cas")).expect("cas acilmali");
        let refs = Arc::new(CasRefcounts::open(&db_path, cas).expect("defter acilmali"));
        let store = MediaStore::open(&db_path, Arc::clone(&refs))
            .expect("depo acilmali")
            .with_ttl(Duration::seconds(1));
        let trigger = SharedTrigger::opened(oturum());
        let rec = VideoRecorder::new();

        rec.start(&trigger).expect("kayit baslamali");
        assert!(rec.push_frame(b"kare".to_vec(), 0));
        let stored = rec
            .stop(&store)
            .expect("durus gecmeli")
            .expect("son segment yazilmali");

        let rapor = store
            .sweep(Utc::now() + Duration::seconds(120))
            .expect("sweep kosmali");
        assert!(rapor.gc.deleted.contains(&stored.blob_ref));
        assert_eq!(rapor.deleted_rows, 1);
        assert_eq!(refs.tracked_count().expect("defter sayimi"), 0);
    }
}
