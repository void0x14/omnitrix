//! Ajan gelen kutusu — MASTER-PLAN 19.1 (6.8 "ajana yazma", Y5).
//!
//! > "Her ajana **hem AI hem kullanici** calisirken prompt yazabilir
//! > (`xai-prompt-queue` -> `omni-control::WriteToAgent`). Subagent
//! > 'ac-unut' degil."
//!
//! Bu modul o cumlenin veri yapisidir: ajan **calisirken** gelen mesajlari
//! ajan basina FIFO bir kuyrukta tutar, tur dongusu her tur basinda kuyrugu
//! bosaltir. Kuyruk satirlari ve birlestirme kurallari `xai-prompt-queue`'dan
//! gelir (I2: salt okuma, kendi kopyasini uretmez):
//!
//! | Bu modul                | `xai-prompt-queue` karsiligi                  |
//! |-------------------------|-----------------------------------------------|
//! | [`InboundMessage`]      | [`QueueEntryMeta`] (aktor ici satir metadata) |
//! | [`AgentInbox::snapshot`]| [`QueueChanged`] + [`QueueEntryWire`] (tel)   |
//! | [`AgentInbox::drain_combined`] | [`combine_prefix_len`] + [`join_texts`] |
//! | [`InboundMessage::content_meta`] | [`stamp_combined_display_texts`]     |
//!
//! ## Yazma yolu
//!
//! [`omni_proto::Command::WriteToAgent`] tam olarak **bu** kuyruga duser;
//! `omni-control` komutu cozer, [`AgentInbox::push_command`] cagirir. TUI,
//! WebUI, Telegram ve ust ajan ayni kapidan gecer — ikinci bir enjeksiyon yolu
//! yoktur (B5 tekrari yasak).
//!
//! ## Okuma yolu
//!
//! `omni-router::turn::TurnLoop` her tur **basinda** [`AgentInbox::drain`] ya
//! da [`AgentInbox::drain_combined`] cagirir; donen mesajlar o turun kullanici
//! bloguna eklenir. Bu modul tur dongusunu tanimaz, yalnizca API sunar.
//!
//! ## Duzenleme kilidi
//!
//! Bir satir kullanici tarafindan duzenlenirken [`AgentInbox::hold`] ile
//! kilitlenir: kilitli satir ne bosaltilir ne de birlestirilir; yarim yazilmis
//! metin modele gitmez. [`AgentInbox::release`] kilidi kaldirir.
//!
//! Uretim yolunda panik yoktur (I6); tum hata yollari [`SchedulerError`] ile
//! tasinir.

use std::collections::VecDeque;

use dashmap::DashMap;
use omni_proto::{AgentId, Command, Timestamp};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};
use uuid::Uuid;
use xai_prompt_queue::{
    CombineGate, QueueChanged, QueueEntryMeta, QueueEntryWire, combine_prefix_len, is_combined,
    join_texts, stamp_combined_display_texts,
};

// ---------------------------------------------------------------------------
// Sabitler
// ---------------------------------------------------------------------------

/// Duz kullanici promptu — birlestirmeye katilabilen tek tur.
pub const KIND_PROMPT: &str = "prompt";

/// Kabuk komutu satiri; birlestirmeyi durdurur.
pub const KIND_BASH: &str = "bash";

/// Ajan basina varsayilan bekleyen mesaj tavani. Sinirsiz kuyruk, calisan bir
/// ajanin baglamini tek turda patlatabilir; tavan geri basinci uretir.
pub const DEFAULT_CAPACITY: usize = 64;

/// Tek mesaj icin varsayilan bayt tavani (128 KiB).
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 128 * 1024;

// ---------------------------------------------------------------------------
// Hatalar
// ---------------------------------------------------------------------------

/// Gelen kutusu islemlerinin uretebilecegi hatalar.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// Ajanin bekleyen mesaj tavani doldu; yazan taraf geri basinc gormeli.
    #[error("ajan {agent_id} gelen kutusu dolu ({capacity} mesaj)")]
    InboxFull {
        /// Hedef ajan.
        agent_id: AgentId,
        /// Yururlukteki tavan.
        capacity: usize,
    },

    /// Ajan bitti ya da kapatildi; yeni mesaj kabul edilmez.
    #[error("ajan {0} gelen kutusu kapali")]
    InboxClosed(AgentId),

    /// Bos ya da yalniz bosluk iceren govde kuyruga girmez.
    #[error("ajan {0} icin bos mesaj govdesi")]
    EmptyMessage(AgentId),

    /// Tek mesaj bayt tavanini asti.
    #[error("ajan {agent_id} icin mesaj cok buyuk: {bytes} bayt (tavan {limit})")]
    MessageTooLarge {
        /// Hedef ajan.
        agent_id: AgentId,
        /// Gelen govde uzunlugu.
        bytes: usize,
        /// Yururlukteki tavan.
        limit: usize,
    },

    /// Ayni `id` ile ikinci kez itildi; tekrar teslim sessizce yutulmaz.
    #[error("ajan {agent_id} kuyrugunda {id} zaten var")]
    DuplicateMessage {
        /// Hedef ajan.
        agent_id: AgentId,
        /// Catisan satir kimligi.
        id: String,
    },

    /// Duzenleme/silme icin verilen satir kimligi kuyrukta yok.
    #[error("ajan {agent_id} kuyrugunda {id} yok")]
    UnknownMessage {
        /// Hedef ajan.
        agent_id: AgentId,
        /// Aranan satir kimligi.
        id: String,
    },

    /// Gelen kutusuna dusmeyen bir komut [`AgentInbox::push_command`]'a verildi.
    #[error("gelen kutusuna dusmeyen komut: {0}")]
    UnsupportedCommand(&'static str),
}

// ---------------------------------------------------------------------------
// Mesaj kaynagi
// ---------------------------------------------------------------------------

/// Mesaji kimin yazdigi. Y5'in "hem AI hem kullanici" kuralinin tip karsiligi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum MessageOrigin {
    /// Insan kullanici; `client` yazan yuz/kanal kimligidir (tui, webui, ...).
    User {
        /// Kuyruk satirinin `owner` alanina yazilan istemci kimligi.
        client: String,
    },

    /// Baska bir ajan (ust ajan, yargic) calisan ajana yaziyor.
    Agent {
        /// Yazan ajan.
        agent_id: AgentId,
    },

    /// Cekirdegin urettigi sentetik mesaj (auto-wake, durtme). Gorunur kuyruga
    /// girmez ve birlestirmeye katilmaz.
    System,
}

impl MessageOrigin {
    /// Kuyruk satirinin `owner` alani; sentetik kaynak icin `None`.
    #[must_use]
    pub fn owner(&self) -> Option<String> {
        match self {
            Self::User { client } => Some(client.clone()),
            Self::Agent { agent_id } => Some(format!("agent:{agent_id}")),
            Self::System => None,
        }
    }

    /// Sentetik kaynak mi? `xai-prompt-queue` birlestirme kapisinin girdisi.
    #[must_use]
    pub const fn is_synthetic(&self) -> bool {
        matches!(self, Self::System)
    }
}

// ---------------------------------------------------------------------------
// Mesaj
// ---------------------------------------------------------------------------

/// Calisan ajana enjekte edilecek tek mesaj.
///
/// Alanlar [`QueueEntryMeta`] ile bire bir eslesir; ek alanlar
/// (`origin`, `enqueued_at`, `has_images`, `expanded_skill`) birlestirme
/// kapisini ([`CombineGate`]) doldurmak icindir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Kararli satir kimligi; kuyruk metadata'sinin `id`'si.
    pub id: String,
    /// Mesaji kim yazdi.
    pub origin: MessageOrigin,
    /// Satir turu; [`KIND_PROMPT`] / [`KIND_BASH`] ya da baska bir etiket.
    pub kind: String,
    /// Modele gidecek govde.
    pub text: String,
    /// Her yerinde duzenlemede artan surum; bayat surumle duzenleme no-op.
    pub version: u64,
    /// Son duzenleyen istemci kimligi.
    pub last_editor: Option<String>,
    /// Birlestirme birden cok promptu tek govdeye katladiginda parcalar.
    pub combined_texts: Option<Vec<String>>,
    /// Govde gorsel tasiyor mu? Takipci satir gorselliyse birlestirme durur.
    pub has_images: bool,
    /// Istemcide acilmis skill yuku mu? Birlestirmeye katilmaz.
    pub expanded_skill: bool,
    /// Kuyruga girme ani.
    pub enqueued_at: Timestamp,
}

impl InboundMessage {
    /// Verilen kaynak ve govde ile yeni bir duz prompt satiri uretir.
    #[must_use]
    pub fn new(origin: MessageOrigin, text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            origin,
            kind: KIND_PROMPT.to_string(),
            text: text.into(),
            version: 0,
            last_editor: None,
            combined_texts: None,
            has_images: false,
            expanded_skill: false,
            enqueued_at: omni_proto::now(),
        }
    }

    /// Insan kullanici mesaji.
    #[must_use]
    pub fn user(client: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(
            MessageOrigin::User {
                client: client.into(),
            },
            text,
        )
    }

    /// Ust ajanin / yargicin yazdigi mesaj.
    #[must_use]
    pub fn from_agent(agent_id: AgentId, text: impl Into<String>) -> Self {
        Self::new(MessageOrigin::Agent { agent_id }, text)
    }

    /// Cekirdegin urettigi sentetik mesaj (gorunur kuyruga girmez).
    #[must_use]
    pub fn system(text: impl Into<String>) -> Self {
        Self::new(MessageOrigin::System, text)
    }

    /// Satir turunu degistirir (ornegin [`KIND_BASH`]).
    #[must_use]
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = kind.into();
        self
    }

    /// Satir kimligini disaridan sabitler (idempotent yeniden teslim icin).
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Gorsel bayragini isaretler.
    #[must_use]
    pub const fn with_images(mut self, has_images: bool) -> Self {
        self.has_images = has_images;
        self
    }

    /// Acilmis skill yuku bayragini isaretler.
    #[must_use]
    pub const fn with_expanded_skill(mut self, expanded: bool) -> Self {
        self.expanded_skill = expanded;
        self
    }

    /// `omni-proto` komutunu gelen kutusu mesajina cevirir.
    ///
    /// Yalnizca [`Command::WriteToAgent`] karsilik bulur; diger komutlar
    /// gelen kutusuna dusmez.
    ///
    /// # Errors
    /// Komut `WriteToAgent` degilse [`SchedulerError::UnsupportedCommand`].
    pub fn from_command(
        cmd: &Command,
        client: &str,
    ) -> Result<(AgentId, Self), SchedulerError> {
        match cmd {
            Command::WriteToAgent { agent_id, content } => {
                Ok((*agent_id, Self::user(client, content.clone())))
            }
            other => Err(SchedulerError::UnsupportedCommand(other.kind())),
        }
    }

    /// `xai-prompt-queue` birlestirme kapisi.
    #[must_use]
    pub fn gate(&self) -> CombineGate<'_> {
        CombineGate {
            id: &self.id,
            is_plain_prompt: self.kind == KIND_PROMPT,
            is_synthetic: self.origin.is_synthetic(),
            is_expanded_skill: self.expanded_skill,
            is_bash: self.kind == KIND_BASH,
            has_images: self.has_images,
            text: &self.text,
        }
    }

    /// Aktor ici kuyruk metadata'si.
    #[must_use]
    pub fn to_meta(&self) -> QueueEntryMeta {
        QueueEntryMeta {
            id: self.id.clone(),
            version: self.version,
            owner: self.origin.owner(),
            last_editor: self.last_editor.clone(),
            kind: self.kind.clone(),
            text: self.text.clone(),
            combined_texts: self.combined_texts.clone(),
        }
    }

    /// Tel uzerindeki kuyruk satiri; `position` bekleyenler icindeki 0-tabanli sira.
    #[must_use]
    pub fn to_wire(&self, position: usize) -> QueueEntryWire {
        QueueEntryWire {
            id: self.id.clone(),
            version: self.version,
            owner: self.origin.owner(),
            last_editor: self.last_editor.clone(),
            kind: self.kind.clone(),
            text: self.text.clone(),
            combined_texts: self.combined_texts.clone(),
            position,
        }
    }

    /// Icerik blogunun `_meta` haritasi. Birlesik govdede (>= 2 parca)
    /// `combinedDisplayTexts` damgasi basilir, aksi halde harita bos kalir.
    #[must_use]
    pub fn content_meta(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut meta = serde_json::Map::new();
        if let Some(segments) = self.combined_texts.as_ref() {
            stamp_combined_display_texts(&mut meta, segments);
        }
        meta
    }

    /// Birden cok prompt katlanmis mi?
    #[must_use]
    pub fn is_combined(&self) -> bool {
        self.combined_texts.as_ref().is_some_and(|s| is_combined(s))
    }
}

// ---------------------------------------------------------------------------
// Surulen prompt
// ---------------------------------------------------------------------------

/// Su an tur dongusune verilmis prompt; [`QueueChanged`]'in `running_*` alanlari.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RunningPrompt {
    id: String,
    kind: String,
    text: String,
    combined_texts: Option<Vec<String>>,
}

impl From<&InboundMessage> for RunningPrompt {
    fn from(msg: &InboundMessage) -> Self {
        Self {
            id: msg.id.clone(),
            kind: msg.kind.clone(),
            text: msg.text.clone(),
            combined_texts: msg.combined_texts.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Ajan basina kuyruk
// ---------------------------------------------------------------------------

/// Tek ajanin kuyruk durumu.
#[derive(Debug, Default)]
struct AgentQueue {
    /// Bekleyen mesajlar, FIFO.
    pending: VecDeque<InboundMessage>,
    /// Duzenleme kilidi altindaki satir kimlikleri.
    held: Vec<String>,
    /// Tur dongusune verilmis prompt, varsa.
    running: Option<RunningPrompt>,
    /// Kapali kutu: ajan bitti, yeni mesaj kabul edilmez.
    closed: bool,
}

impl AgentQueue {
    fn is_held(&self, id: &str) -> bool {
        self.held.iter().any(|h| h == id)
    }
}

// ---------------------------------------------------------------------------
// Gelen kutusu
// ---------------------------------------------------------------------------

/// Ajan basina prompt kuyrugu (19.1).
///
/// `&self` ile yazilir: `omni-control` dispatcher'i, TUI ve tur dongusu ayni
/// `Arc<AgentInbox>` uzerinden calisir; kilit ajan basinadir (`DashMap`), bir
/// ajanin kuyrugu digerini bekletmez.
///
/// ```
/// use omni_scheduler::inbox::{AgentInbox, InboundMessage};
///
/// let inbox = AgentInbox::new();
/// inbox.push(7, InboundMessage::user("tui", "sema degisti, dosyayi tekrar oku"))?;
/// let batch = inbox.drain(7);
/// assert_eq!(batch.len(), 1);
/// # Ok::<(), omni_scheduler::inbox::SchedulerError>(())
/// ```
#[derive(Debug)]
pub struct AgentInbox {
    queues: DashMap<AgentId, AgentQueue>,
    capacity: usize,
    max_message_bytes: usize,
}

impl Default for AgentInbox {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentInbox {
    /// Varsayilan tavanlarla bos gelen kutusu.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_CAPACITY, DEFAULT_MAX_MESSAGE_BYTES)
    }

    /// Tavanlari acikca veren kurucu.
    #[must_use]
    pub fn with_limits(capacity: usize, max_message_bytes: usize) -> Self {
        Self {
            queues: DashMap::new(),
            capacity: capacity.max(1),
            max_message_bytes: max_message_bytes.max(1),
        }
    }

    /// Ajan basina bekleyen mesaj tavani.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    // -- yazma ------------------------------------------------------------

    /// Calisan ajana mesaj enjekte eder.
    ///
    /// # Errors
    /// - [`SchedulerError::InboxClosed`] — ajan kapatilmis.
    /// - [`SchedulerError::EmptyMessage`] — govde bos / yalniz bosluk.
    /// - [`SchedulerError::MessageTooLarge`] — bayt tavani asildi.
    /// - [`SchedulerError::DuplicateMessage`] — ayni `id` zaten kuyrukta.
    /// - [`SchedulerError::InboxFull`] — bekleyen mesaj tavani doldu.
    pub fn push(&self, agent_id: AgentId, msg: InboundMessage) -> Result<(), SchedulerError> {
        if msg.text.trim().is_empty() {
            return Err(SchedulerError::EmptyMessage(agent_id));
        }
        let bytes = msg.text.len();
        if bytes > self.max_message_bytes {
            return Err(SchedulerError::MessageTooLarge {
                agent_id,
                bytes,
                limit: self.max_message_bytes,
            });
        }

        let mut entry = self.queues.entry(agent_id).or_default();
        let queue = entry.value_mut();

        if queue.closed {
            return Err(SchedulerError::InboxClosed(agent_id));
        }
        if queue.pending.iter().any(|m| m.id == msg.id) {
            return Err(SchedulerError::DuplicateMessage {
                agent_id,
                id: msg.id,
            });
        }
        if queue.pending.len() >= self.capacity {
            warn!(
                agent_id,
                capacity = self.capacity,
                "gelen kutusu dolu, mesaj reddedildi"
            );
            return Err(SchedulerError::InboxFull {
                agent_id,
                capacity: self.capacity,
            });
        }

        debug!(
            agent_id,
            id = %msg.id,
            kind = %msg.kind,
            depth = queue.pending.len() + 1,
            "ajana mesaj enjekte edildi"
        );
        queue.pending.push_back(msg);
        Ok(())
    }

    /// [`Command::WriteToAgent`] komutunu dogrudan kuyruga dusurur.
    ///
    /// `omni-control` dispatcher'inin cagirdigi kapi: komuttan hedef ajan ve
    /// govde cozulur, [`Self::push`] uygulanir.
    ///
    /// # Errors
    /// Komut `WriteToAgent` degilse [`SchedulerError::UnsupportedCommand`];
    /// kuyruk hatalari [`Self::push`] ile aynidir.
    pub fn push_command(&self, cmd: &Command, client: &str) -> Result<(), SchedulerError> {
        let (agent_id, msg) = InboundMessage::from_command(cmd, client)?;
        self.push(agent_id, msg)
    }

    // -- okuma ------------------------------------------------------------

    /// Ajanin bekleyen tum mesajlarini FIFO sirada alir ve kuyrugu bosaltir.
    ///
    /// Duzenleme kilidi altindaki satirlar ([`Self::hold`]) kuyrukta kalir.
    /// Tur dongusunun her tur basinda cagirdigi metot budur.
    #[must_use]
    pub fn drain(&self, agent_id: AgentId) -> Vec<InboundMessage> {
        let Some(mut entry) = self.queues.get_mut(&agent_id) else {
            return Vec::new();
        };
        let queue = entry.value_mut();

        let mut taken = Vec::with_capacity(queue.pending.len());
        let mut kept = VecDeque::new();
        while let Some(msg) = queue.pending.pop_front() {
            if queue.held.iter().any(|h| h == &msg.id) {
                kept.push_back(msg);
            } else {
                taken.push(msg);
            }
        }
        queue.pending = kept;

        if let Some(first) = taken.first() {
            queue.running = Some(RunningPrompt::from(first));
            debug!(agent_id, count = taken.len(), "gelen kutusu bosaltildi");
        }
        taken
    }

    /// Kuyrugun onundeki birlesebilir kosuyu **tek** mesaja katlayarak alir.
    ///
    /// Birlestirme kurallari `xai-prompt-queue::combine_prefix_len`'dir: duz
    /// kullanici promptlari birlesir; kabuk satiri, acilmis skill, sentetik
    /// kaynak, gorselli takipci ya da duzenleme kilidi kosuyu keser. Geri kalan
    /// satirlar kuyrukta bekler.
    ///
    /// `[ui].combine_queued_prompts` kapali oldugunda cagiran taraf bunun
    /// yerine [`Self::drain`] kullanir.
    #[must_use]
    pub fn drain_combined(&self, agent_id: AgentId) -> Option<InboundMessage> {
        let mut entry = self.queues.get_mut(&agent_id)?;
        let queue = entry.value_mut();

        // On satir duzenleme kilidi altindaysa hicbir sey surulmez.
        let front_held = queue
            .pending
            .front()
            .is_some_and(|m| queue.held.iter().any(|h| h == &m.id));
        if front_held {
            return None;
        }

        let take = {
            let skip: Vec<&str> = queue.held.iter().map(String::as_str).collect();
            combine_prefix_len(queue.pending.iter().map(InboundMessage::gate), &skip)
        };
        if take == 0 {
            return None;
        }

        let mut run: Vec<InboundMessage> = Vec::with_capacity(take);
        for _ in 0..take {
            match queue.pending.pop_front() {
                Some(msg) => run.push(msg),
                // Uzunluk yukarida olculdu; yine de panik yok (I6).
                None => break,
            }
        }

        let mut merged = run.first().cloned()?;
        if run.len() >= 2 {
            let mut segments: Vec<String> = Vec::with_capacity(run.len());
            for msg in &run {
                match msg.combined_texts.as_ref() {
                    Some(prev) => segments.extend(prev.iter().cloned()),
                    None => segments.push(msg.text.clone()),
                }
            }
            merged.text = join_texts(segments.iter().map(String::as_str));
            merged.combined_texts = Some(segments);
            debug!(
                agent_id,
                merged = run.len(),
                "bekleyen promptlar tek govdede birlestirildi"
            );
        }

        queue.running = Some(RunningPrompt::from(&merged));
        Some(merged)
    }

    /// Kuyrugu bozmadan bekleyen mesajlarin kopyasini verir.
    #[must_use]
    pub fn peek(&self, agent_id: AgentId) -> Vec<InboundMessage> {
        self.queues
            .get(&agent_id)
            .map(|q| q.pending.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Ajanin bekleyen mesaj sayisi.
    #[must_use]
    pub fn len(&self, agent_id: AgentId) -> usize {
        self.queues.get(&agent_id).map_or(0, |q| q.pending.len())
    }

    /// Ajanin bekleyen mesaji var mi?
    #[must_use]
    pub fn is_empty(&self, agent_id: AgentId) -> bool {
        self.len(agent_id) == 0
    }

    /// Tum ajanlardaki toplam bekleyen mesaj sayisi (gosterge/olcum icin).
    #[must_use]
    pub fn total_pending(&self) -> usize {
        self.queues.iter().map(|q| q.pending.len()).sum()
    }

    /// Kuyrugu olan ajanlarin kimlikleri.
    #[must_use]
    pub fn agents(&self) -> Vec<AgentId> {
        self.queues.iter().map(|q| *q.key()).collect()
    }

    // -- duzenleme --------------------------------------------------------

    /// Bekleyen satiri yerinde duzenler; surum bayatsa islem **no-op**'tur.
    ///
    /// Donen deger duzenlemenin uygulanip uygulanmadigidir.
    ///
    /// # Errors
    /// - [`SchedulerError::EmptyMessage`] — yeni govde bos.
    /// - [`SchedulerError::MessageTooLarge`] — bayt tavani asildi.
    /// - [`SchedulerError::UnknownMessage`] — satir kuyrukta yok.
    pub fn edit(
        &self,
        agent_id: AgentId,
        id: &str,
        expected_version: u64,
        text: impl Into<String>,
        editor: &str,
    ) -> Result<bool, SchedulerError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(SchedulerError::EmptyMessage(agent_id));
        }
        if text.len() > self.max_message_bytes {
            return Err(SchedulerError::MessageTooLarge {
                agent_id,
                bytes: text.len(),
                limit: self.max_message_bytes,
            });
        }

        let mut entry =
            self.queues
                .get_mut(&agent_id)
                .ok_or_else(|| SchedulerError::UnknownMessage {
                    agent_id,
                    id: id.to_string(),
                })?;
        let queue = entry.value_mut();

        let Some(msg) = queue.pending.iter_mut().find(|m| m.id == id) else {
            return Err(SchedulerError::UnknownMessage {
                agent_id,
                id: id.to_string(),
            });
        };
        if msg.version != expected_version {
            debug!(
                agent_id,
                id,
                have = msg.version,
                got = expected_version,
                "bayat surumle duzenleme yok sayildi"
            );
            return Ok(false);
        }

        msg.text = text;
        msg.combined_texts = None;
        msg.version = msg.version.saturating_add(1);
        msg.last_editor = Some(editor.to_string());
        Ok(true)
    }

    /// Bekleyen satiri kuyruktan cikarir.
    ///
    /// # Errors
    /// Satir kuyrukta yoksa [`SchedulerError::UnknownMessage`].
    pub fn remove(&self, agent_id: AgentId, id: &str) -> Result<InboundMessage, SchedulerError> {
        let mut entry =
            self.queues
                .get_mut(&agent_id)
                .ok_or_else(|| SchedulerError::UnknownMessage {
                    agent_id,
                    id: id.to_string(),
                })?;
        let queue = entry.value_mut();

        let Some(pos) = queue.pending.iter().position(|m| m.id == id) else {
            return Err(SchedulerError::UnknownMessage {
                agent_id,
                id: id.to_string(),
            });
        };
        queue.held.retain(|h| h != id);
        queue
            .pending
            .remove(pos)
            .ok_or_else(|| SchedulerError::UnknownMessage {
                agent_id,
                id: id.to_string(),
            })
    }

    /// Satiri duzenleme kilidi altina alir: bosaltilmaz, birlestirilmez.
    pub fn hold(&self, agent_id: AgentId, id: &str) {
        let mut entry = self.queues.entry(agent_id).or_default();
        let queue = entry.value_mut();
        if !queue.is_held(id) {
            queue.held.push(id.to_string());
        }
    }

    /// Duzenleme kilidini kaldirir.
    pub fn release(&self, agent_id: AgentId, id: &str) {
        if let Some(mut entry) = self.queues.get_mut(&agent_id) {
            entry.value_mut().held.retain(|h| h != id);
        }
    }

    /// Satir duzenleme kilidi altinda mi?
    #[must_use]
    pub fn is_held(&self, agent_id: AgentId, id: &str) -> bool {
        self.queues.get(&agent_id).is_some_and(|q| q.is_held(id))
    }

    // -- yasam dongusu ----------------------------------------------------

    /// Ajan icin kuyrugu acar (yoksa yaratir, kapaliysa yeniden acar).
    pub fn open(&self, agent_id: AgentId) {
        let mut entry = self.queues.entry(agent_id).or_default();
        entry.value_mut().closed = false;
    }

    /// Kuyrugu kapatir: bekleyenler durur, yeni mesaj reddedilir.
    pub fn close(&self, agent_id: AgentId) {
        let mut entry = self.queues.entry(agent_id).or_default();
        entry.value_mut().closed = true;
    }

    /// Kuyruk kapali mi?
    #[must_use]
    pub fn is_closed(&self, agent_id: AgentId) -> bool {
        self.queues.get(&agent_id).is_some_and(|q| q.closed)
    }

    /// Turun bitisini isaretler: surulen prompt kaydi silinir.
    pub fn finish_turn(&self, agent_id: AgentId) {
        if let Some(mut entry) = self.queues.get_mut(&agent_id) {
            entry.value_mut().running = None;
        }
    }

    /// Ajanin kuyrugunu tumden unutur (ajan sonlandi / temizlik).
    pub fn forget(&self, agent_id: AgentId) {
        self.queues.remove(&agent_id);
    }

    // -- tel --------------------------------------------------------------

    /// Ajanin kuyrugunu `x.ai/queue/changed` yayin govdesine cevirir.
    ///
    /// Surulen satir `entries`'te **yer almaz**; `running_*` alanlarinda tasinir.
    #[must_use]
    pub fn snapshot(&self, agent_id: AgentId, session_id: impl Into<String>) -> QueueChanged {
        let session_id = session_id.into();
        let Some(queue) = self.queues.get(&agent_id) else {
            return QueueChanged {
                session_id,
                ..QueueChanged::default()
            };
        };

        let entries = queue
            .pending
            .iter()
            .filter(|m| !m.origin.is_synthetic())
            .enumerate()
            .map(|(position, msg)| msg.to_wire(position))
            .collect();

        let running = queue.running.as_ref();
        QueueChanged {
            session_id,
            entries,
            running_prompt_id: running.map(|r| r.id.clone()),
            running_text: running.map(|r| r.text.clone()),
            running_kind: running.map(|r| r.kind.clone()),
            running_combined_texts: running.and_then(|r| r.combined_texts.clone()),
        }
    }

    /// Bekleyen satirlarin aktor ici metadata gorunumu.
    #[must_use]
    pub fn metas(&self, agent_id: AgentId) -> Vec<QueueEntryMeta> {
        self.queues
            .get(&agent_id)
            .map(|q| q.pending.iter().map(InboundMessage::to_meta).collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const A: AgentId = 7;

    #[test]
    fn push_then_drain_is_fifo() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "iki")).unwrap();
        let batch = inbox.drain(A);
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].text, "bir");
        assert_eq!(batch[1].text, "iki");
        assert!(inbox.is_empty(A));
        // Ikinci bosaltma bostur.
        assert!(inbox.drain(A).is_empty());
    }

    #[test]
    fn drain_of_unknown_agent_is_empty() {
        let inbox = AgentInbox::new();
        assert!(inbox.drain(999).is_empty());
        assert_eq!(inbox.len(999), 0);
    }

    #[test]
    fn ai_and_user_share_the_same_queue() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("webui", "kullanici")).unwrap();
        inbox.push(A, InboundMessage::from_agent(3, "ust ajan")).unwrap();
        let batch = inbox.drain(A);
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].origin.owner().as_deref(), Some("webui"));
        assert_eq!(batch[1].origin.owner().as_deref(), Some("agent:3"));
    }

    #[test]
    fn write_to_agent_command_lands_in_inbox() {
        let inbox = AgentInbox::new();
        let cmd = Command::WriteToAgent {
            agent_id: A,
            content: "sema degisti".into(),
        };
        inbox.push_command(&cmd, "telegram").unwrap();
        let batch = inbox.drain(A);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].text, "sema degisti");
        assert_eq!(batch[0].origin.owner().as_deref(), Some("telegram"));
    }

    #[test]
    fn non_write_command_is_rejected() {
        let inbox = AgentInbox::new();
        let cmd = Command::Interrupt {
            agent_id: A,
            kind: "user".into(),
            source: "tui".into(),
            reason: None,
        };
        let err = inbox.push_command(&cmd, "tui").unwrap_err();
        assert!(matches!(
            err,
            SchedulerError::UnsupportedCommand("interrupt")
        ));
    }

    #[test]
    fn empty_and_oversized_messages_are_rejected() {
        let inbox = AgentInbox::with_limits(8, 16);
        let err = inbox.push(A, InboundMessage::user("tui", "   ")).unwrap_err();
        assert!(matches!(err, SchedulerError::EmptyMessage(7)));

        let big = "x".repeat(17);
        let err = inbox.push(A, InboundMessage::user("tui", big)).unwrap_err();
        assert!(matches!(err, SchedulerError::MessageTooLarge { bytes: 17, .. }));
    }

    #[test]
    fn capacity_produces_backpressure() {
        let inbox = AgentInbox::with_limits(2, 1024);
        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "b")).unwrap();
        let err = inbox.push(A, InboundMessage::user("tui", "c")).unwrap_err();
        assert!(matches!(err, SchedulerError::InboxFull { capacity: 2, .. }));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let inbox = AgentInbox::new();
        inbox
            .push(A, InboundMessage::user("tui", "a").with_id("fixed"))
            .unwrap();
        let err = inbox
            .push(A, InboundMessage::user("tui", "b").with_id("fixed"))
            .unwrap_err();
        assert!(matches!(err, SchedulerError::DuplicateMessage { .. }));
    }

    #[test]
    fn closed_inbox_rejects_writes() {
        let inbox = AgentInbox::new();
        inbox.close(A);
        assert!(inbox.is_closed(A));
        let err = inbox.push(A, InboundMessage::user("tui", "a")).unwrap_err();
        assert!(matches!(err, SchedulerError::InboxClosed(7)));
        inbox.open(A);
        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
    }

    #[test]
    fn held_row_survives_drain() {
        let inbox = AgentInbox::new();
        inbox
            .push(A, InboundMessage::user("tui", "a").with_id("keep"))
            .unwrap();
        inbox.push(A, InboundMessage::user("tui", "b")).unwrap();
        inbox.hold(A, "keep");
        assert!(inbox.is_held(A, "keep"));

        let batch = inbox.drain(A);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].text, "b");
        assert_eq!(inbox.len(A), 1);

        inbox.release(A, "keep");
        assert_eq!(inbox.drain(A).len(), 1);
    }

    #[test]
    fn combine_merges_plain_prompt_run() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "iki")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "uc")).unwrap();

        let merged = inbox.drain_combined(A).unwrap();
        assert_eq!(merged.text, "bir\n\niki\n\nuc");
        assert!(merged.is_combined());
        assert_eq!(
            merged.combined_texts.as_deref(),
            Some(["bir".to_string(), "iki".to_string(), "uc".to_string()].as_slice())
        );
        assert!(inbox.is_empty(A));
    }

    #[test]
    fn combine_stops_at_bash_row() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();
        inbox
            .push(A, InboundMessage::user("tui", "ls -la").with_kind(KIND_BASH))
            .unwrap();
        inbox.push(A, InboundMessage::user("tui", "uc")).unwrap();

        let merged = inbox.drain_combined(A).unwrap();
        assert_eq!(merged.text, "bir");
        assert!(!merged.is_combined());
        assert_eq!(inbox.len(A), 2);
    }

    #[test]
    fn combine_stops_at_held_follower() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();
        inbox
            .push(A, InboundMessage::user("tui", "taslak").with_id("edit"))
            .unwrap();
        inbox.hold(A, "edit");

        let merged = inbox.drain_combined(A).unwrap();
        assert_eq!(merged.text, "bir");
        assert_eq!(inbox.len(A), 1);
    }

    #[test]
    fn combine_returns_none_when_front_is_held() {
        let inbox = AgentInbox::new();
        inbox
            .push(A, InboundMessage::user("tui", "taslak").with_id("edit"))
            .unwrap();
        inbox.hold(A, "edit");
        assert!(inbox.drain_combined(A).is_none());
    }

    #[test]
    fn combine_on_empty_queue_is_none() {
        let inbox = AgentInbox::new();
        assert!(inbox.drain_combined(A).is_none());
        inbox.open(A);
        assert!(inbox.drain_combined(A).is_none());
    }

    #[test]
    fn synthetic_message_is_not_combined_and_hidden_from_wire() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::system("auto-wake")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();

        let snap = inbox.snapshot(A, "sess-1");
        assert_eq!(snap.session_id, "sess-1");
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.entries[0].text, "bir");

        let merged = inbox.drain_combined(A).unwrap();
        assert_eq!(merged.text, "auto-wake");
        assert!(!merged.is_combined());
    }

    #[test]
    fn edit_bumps_version_and_stale_edit_is_noop() {
        let inbox = AgentInbox::new();
        inbox
            .push(A, InboundMessage::user("tui", "eski").with_id("m1"))
            .unwrap();

        assert!(inbox.edit(A, "m1", 0, "yeni", "webui").unwrap());
        let rows = inbox.peek(A);
        assert_eq!(rows[0].text, "yeni");
        assert_eq!(rows[0].version, 1);
        assert_eq!(rows[0].last_editor.as_deref(), Some("webui"));

        // Bayat surum: no-op, hata degil.
        assert!(!inbox.edit(A, "m1", 0, "daha yeni", "tui").unwrap());
        assert_eq!(inbox.peek(A)[0].text, "yeni");
    }

    #[test]
    fn edit_unknown_row_errors() {
        let inbox = AgentInbox::new();
        let err = inbox.edit(A, "yok", 0, "x", "tui").unwrap_err();
        assert!(matches!(err, SchedulerError::UnknownMessage { .. }));

        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
        let err = inbox.edit(A, "yok", 0, "x", "tui").unwrap_err();
        assert!(matches!(err, SchedulerError::UnknownMessage { .. }));
    }

    #[test]
    fn remove_takes_row_out() {
        let inbox = AgentInbox::new();
        inbox
            .push(A, InboundMessage::user("tui", "a").with_id("m1"))
            .unwrap();
        inbox.push(A, InboundMessage::user("tui", "b")).unwrap();
        inbox.hold(A, "m1");

        let gone = inbox.remove(A, "m1").unwrap();
        assert_eq!(gone.text, "a");
        assert!(!inbox.is_held(A, "m1"));
        assert_eq!(inbox.len(A), 1);
        assert!(inbox.remove(A, "m1").is_err());
    }

    #[test]
    fn snapshot_carries_running_prompt() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "iki")).unwrap();
        let merged = inbox.drain_combined(A).unwrap();

        let snap = inbox.snapshot(A, "sess-9");
        assert!(snap.entries.is_empty());
        assert_eq!(snap.running_prompt_id.as_deref(), Some(merged.id.as_str()));
        assert_eq!(snap.running_text.as_deref(), Some("bir\n\niki"));
        assert_eq!(snap.running_kind.as_deref(), Some(KIND_PROMPT));
        assert_eq!(
            snap.running_combined_texts.as_deref(),
            Some(["bir".to_string(), "iki".to_string()].as_slice())
        );

        inbox.finish_turn(A);
        assert!(inbox.snapshot(A, "sess-9").running_prompt_id.is_none());
    }

    #[test]
    fn snapshot_of_unknown_agent_is_empty() {
        let inbox = AgentInbox::new();
        let snap = inbox.snapshot(404, "sess-x");
        assert_eq!(snap.session_id, "sess-x");
        assert!(snap.entries.is_empty());
        assert!(snap.running_prompt_id.is_none());
    }

    #[test]
    fn wire_positions_are_zero_based() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "b")).unwrap();
        let snap = inbox.snapshot(A, "s");
        assert_eq!(snap.entries[0].position, 0);
        assert_eq!(snap.entries[1].position, 1);
        assert_eq!(snap.entries[0].owner.as_deref(), Some("tui"));
    }

    #[test]
    fn content_meta_stamped_only_for_combined_bodies() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "tek")).unwrap();
        let single = inbox.drain_combined(A).unwrap();
        assert!(single.content_meta().is_empty());

        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "b")).unwrap();
        let merged = inbox.drain_combined(A).unwrap();
        let meta = merged.content_meta();
        assert_eq!(
            meta.get(xai_prompt_queue::COMBINED_DISPLAY_TEXTS_META),
            Some(&serde_json::json!(["a", "b"]))
        );
    }

    #[test]
    fn metas_mirror_pending_rows() {
        let inbox = AgentInbox::new();
        inbox
            .push(A, InboundMessage::user("tui", "a").with_id("m1"))
            .unwrap();
        let metas = inbox.metas(A);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].id, "m1");
        assert_eq!(metas[0].kind, KIND_PROMPT);
        assert_eq!(metas[0].owner.as_deref(), Some("tui"));
    }

    #[test]
    fn forget_and_totals() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
        inbox.push(8, InboundMessage::user("tui", "b")).unwrap();
        assert_eq!(inbox.total_pending(), 2);
        let mut ids = inbox.agents();
        ids.sort_unstable();
        assert_eq!(ids, vec![7, 8]);

        inbox.forget(A);
        assert_eq!(inbox.total_pending(), 1);
        assert_eq!(inbox.len(A), 0);
    }

    #[test]
    fn expanded_skill_and_image_gates_stop_the_run() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "bir")).unwrap();
        inbox
            .push(
                A,
                InboundMessage::user("tui", "gorsel").with_images(true),
            )
            .unwrap();
        assert_eq!(inbox.drain_combined(A).unwrap().text, "bir");

        let inbox2 = AgentInbox::new();
        inbox2
            .push(
                A,
                InboundMessage::user("tui", "/commit").with_expanded_skill(true),
            )
            .unwrap();
        inbox2.push(A, InboundMessage::user("tui", "iki")).unwrap();
        assert_eq!(inbox2.drain_combined(A).unwrap().text, "/commit");
    }

    #[test]
    fn merged_body_flattens_previous_segments() {
        let inbox = AgentInbox::new();
        inbox.push(A, InboundMessage::user("tui", "a")).unwrap();
        inbox.push(A, InboundMessage::user("tui", "b")).unwrap();
        let first = inbox.drain_combined(A).unwrap();

        // Birlesik govde tekrar kuyruga girer, ustune yeni prompt gelir.
        let inbox2 = AgentInbox::new();
        inbox2.push(A, first).unwrap();
        inbox2.push(A, InboundMessage::user("tui", "c")).unwrap();
        let second = inbox2.drain_combined(A).unwrap();
        assert_eq!(second.text, "a\n\nb\n\nc");
        assert_eq!(
            second.combined_texts.as_deref(),
            Some(["a".to_string(), "b".to_string(), "c".to_string()].as_slice())
        );
    }
}
