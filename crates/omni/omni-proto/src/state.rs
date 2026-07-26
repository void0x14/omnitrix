//! Kanonik durum tipleri (MASTER-PLAN 6.1).
//!
//! Alan tipleri `migrations/*.sql` ile birebir uyumludur:
//! `0001` providers · `0002` provider_health · `0003` tasks/agents ·
//! `0004` tool_calls · `0005` interrupts · `0008` file_touches.

use serde::{Deserialize, Serialize};

use crate::{
    AgentId, EventSeq, FileTouchId, InterruptId, ProviderId, TaskId, Timestamp, ToolCallId,
};

/// Sistemin o andaki tam goruntusu. UI ilk yuklemede bunu alir, sonrasinda
/// `StateEvent` akisiyla gunceller (Bolum 6.2 okuma sozlesmesi).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSnapshot {
    /// Var-olan ajanlar (tier ayrimi `AgentView::tier` icinde).
    pub agents: Vec<AgentView>,
    /// Gorev agaci duz liste olarak; hiyerarsi `parent_id`/`root_id` ile kurulur.
    pub tasks: Vec<TaskView>,
    /// Saglayicilar ve son saglik okumalari.
    pub providers: Vec<ProviderView>,
    /// Kaynak valisi olcumu (7.2).
    pub resource: ResourceGauge,
    /// Goruntunun alindigi an.
    pub ts: Timestamp,
}

impl SystemSnapshot {
    /// Bos bir goruntu uretir (cold-start, henuz hicbir satir yokken).
    #[must_use]
    pub fn empty(ts: Timestamp) -> Self {
        Self {
            agents: Vec::new(),
            tasks: Vec::new(),
            providers: Vec::new(),
            resource: ResourceGauge::at(ts),
            ts,
        }
    }

    /// Verilen kimlige sahip ajani arar.
    #[must_use]
    pub fn agent(&self, id: AgentId) -> Option<&AgentView> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Verilen kimlige sahip gorevi arar.
    #[must_use]
    pub fn task(&self, id: TaskId) -> Option<&TaskView> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Su an `Active` tier'da olan ajan sayisi.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|a| a.tier == AgentTier::Active)
            .count()
    }
}

/// Tek bir ajanin UI'ya acilan goruntusu (`agents` + `messages` toplami).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentView {
    /// `agents.id`.
    pub id: AgentId,
    /// `agents.persona` — disk uzerindeki persona dosyasinin adi (K12).
    pub persona: String,
    /// `agents.tier` (3.1).
    pub tier: AgentTier,
    /// `agents.task_id`.
    pub task_id: TaskId,
    /// `agents.parent_agent_id`.
    pub parent_id: Option<AgentId>,
    /// `agents.state`.
    pub state: AgentState,
    /// `agents.rss_kb` — olculen yerlesik bellek (7.2 raporlama).
    pub rss_kb: u64,
    /// `messages.tokens_in` toplami.
    pub tokens_in: u64,
    /// `messages.tokens_out` toplami.
    pub tokens_out: u64,
    /// `messages.cost` toplami.
    pub cost: f64,
    /// `trust_scores.score` — `subject_kind = 'agent'` (3.4).
    pub trust: f32,
    /// `agents.depth` — rekursiyon derinligi (7.3).
    pub depth: u8,
    /// Bu goruntunun turedigi son `agent_events.seq`; UI akis bosluklarini
    /// bununla tespit eder.
    pub last_event_seq: EventSeq,
}

impl AgentView {
    /// Ajanin RAM'de bagalam tutup tutmadigi (Queued/Active).
    #[must_use]
    pub fn is_resident(&self) -> bool {
        self.tier.is_resident()
    }

    /// Ajanin daha fazla ilerlemeyeceginin kesinlestigi durum.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

/// Ajan katmani (3.1 — "aktif != var-olan").
///
/// SQLite karsiligi: `agents.tier CHECK(tier IN
/// ('existing','sleeping','queued','active'))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTier {
    /// Yalnizca DB satiri; hic isinmamis.
    Existing,
    /// Baglam CAS'a sikistirilmis, ~200 byte metadata.
    Sleeping,
    /// Calismaya hazir, slot bekliyor.
    Queued,
    /// Vali slot verdi, in-flight.
    Active,
}

impl AgentTier {
    /// SQLite `TEXT` degerinin kanonik hali.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Sleeping => "sleeping",
            Self::Queued => "queued",
            Self::Active => "active",
        }
    }

    /// SQLite `TEXT` degerinden cozer.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "existing" => Some(Self::Existing),
            "sleeping" => Some(Self::Sleeping),
            "queued" => Some(Self::Queued),
            "active" => Some(Self::Active),
            _ => None,
        }
    }

    /// Baglami RAM'de mi duruyor?
    #[must_use]
    pub fn is_resident(self) -> bool {
        matches!(self, Self::Queued | Self::Active)
    }
}

/// Ajanin tur icindeki calisma durumu (`agents.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Kuruldu, henuz ilk turu baslamadi.
    Init,
    /// Plan uretiyor.
    Planning,
    /// Model yanitini bekliyor.
    AwaitingModel,
    /// Tool calistiriyor.
    RunningTool,
    /// Disaridan mudahale edildi (6.8 / AS2).
    Interrupted,
    /// Insan onayi ya da bagimlilik bekliyor.
    Blocked,
    /// Sonlanma oracle'i (7.5) gecti.
    Done,
    /// Kurtarilamaz hata.
    Failed,
}

impl AgentState {
    /// SQLite `TEXT` degerinin kanonik hali.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Planning => "planning",
            Self::AwaitingModel => "awaiting_model",
            Self::RunningTool => "running_tool",
            Self::Interrupted => "interrupted",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// SQLite `TEXT` degerinden cozer.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "init" => Some(Self::Init),
            "planning" => Some(Self::Planning),
            "awaiting_model" => Some(Self::AwaitingModel),
            "running_tool" => Some(Self::RunningTool),
            "interrupted" => Some(Self::Interrupted),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Daha fazla ilerleme beklenmeyen durumlar.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// Gorev goruntusu (`tasks`, 0003 + 0008 revizyonu).
///
/// `mode` ve `status` semada kisitsiz `TEXT`'tir; burada da metin olarak
/// tasinir ki config'ten gelen yeni degerler derleme gerektirmesin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskView {
    /// `tasks.id`.
    pub id: TaskId,
    /// `tasks.parent_id`.
    pub parent_id: Option<TaskId>,
    /// `tasks.root_id`.
    pub root_id: TaskId,
    /// `tasks.title`.
    pub title: String,
    /// `tasks.mode`.
    pub mode: String,
    /// `tasks.status`.
    pub status: String,
    /// `tasks.depth`.
    pub depth: u8,
    /// `tasks.budget_allocated` — ebeveynden devralinan zarf (AS4).
    pub budget_allocated: Option<f64>,
    /// `tasks.budget_spent` — bu gorev ve alt agacin harcamasi (AS4).
    pub budget_spent: Option<f64>,
    /// `tasks.duration_target` (AS13).
    pub duration_target: Option<String>,
    /// `tasks.created_at`.
    pub created_at: Timestamp,
    /// `tasks.closed_at`.
    pub closed_at: Option<Timestamp>,
}

impl TaskView {
    /// Zarf tanimliysa kalan butce; degilse `None` (tam-otonom modda sinirsiz).
    #[must_use]
    pub fn budget_remaining(&self) -> Option<f64> {
        self.budget_allocated
            .map(|tahsis| tahsis - self.budget_spent.unwrap_or(0.0))
    }

    /// Kok gorev mi?
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }
}

/// Saglayici goruntusu (`providers` + son `provider_health` satiri).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderView {
    /// `providers.id`.
    pub id: ProviderId,
    /// `providers.name`.
    pub name: String,
    /// `providers.kind` — config'ten gelir, derlemeye gomulu degil (AS7/I5).
    pub kind: String,
    /// `providers.base_url`.
    pub base_url: String,
    /// `provider_health.state`.
    pub health: ProviderHealthState,
    /// `provider_health.model` — olcumun hangi model uzerinden alindigi.
    pub health_model: Option<String>,
    /// `provider_health.latency_ms`.
    pub latency_ms: Option<u32>,
    /// `provider_health.checked_at`.
    pub checked_at: Option<Timestamp>,
    /// `provider_health.detail`.
    pub detail: Option<String>,
    /// `api_keys` icinde `status = 'active'` olan anahtar sayisi.
    pub active_keys: u32,
    /// `models` tablosunda bu saglayiciya bagli model adlari.
    pub models: Vec<String>,
}

impl ProviderView {
    /// Yonlendirme zincirinde kalabilir mi (10.1)?
    #[must_use]
    pub fn is_routable(&self) -> bool {
        self.health.is_usable() && self.active_keys > 0
    }
}

/// Saglayici canlilik durumu.
///
/// SQLite karsiligi: `provider_health.state CHECK(state IN
/// ('healthy','degraded','down','quota_exhausted'))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthState {
    /// Saglikli.
    Healthy,
    /// Yavas ya da kismi hatali.
    Degraded,
    /// Erisilemiyor.
    Down,
    /// Kota/bakiye bitti.
    QuotaExhausted,
}

impl ProviderHealthState {
    /// SQLite `TEXT` degerinin kanonik hali.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Down => "down",
            Self::QuotaExhausted => "quota_exhausted",
        }
    }

    /// SQLite `TEXT` degerinden cozer.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "healthy" => Some(Self::Healthy),
            "degraded" => Some(Self::Degraded),
            "down" => Some(Self::Down),
            "quota_exhausted" => Some(Self::QuotaExhausted),
            _ => None,
        }
    }

    /// Cagri yapilabilir mi?
    #[must_use]
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

/// Kaynak valisi olcumu (7.2). Kod tavan koymaz; olculen basinci tasir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceGauge {
    /// Surecin toplam yerlesik bellegi.
    pub rss_kb: u64,
    /// Profilden gelen yumusak tavan; yoksa `None`.
    pub rss_limit_kb: Option<u64>,
    /// Anlik CPU kullanimi (yuzde).
    pub cpu_pct: f32,
    /// Acik dosya tanimlayici sayisi.
    pub open_fds: u32,
    /// Isletim sisteminden okunan FD tavani.
    pub fd_limit: Option<u32>,
    /// `Active` tier ajan sayisi.
    pub active: u32,
    /// `Queued` tier ajan sayisi.
    pub queued: u32,
    /// `Sleeping` tier ajan sayisi.
    pub sleeping: u32,
    /// `Existing` tier ajan sayisi.
    pub existing: u32,
    /// Vali yeni `Active` alimina izin veriyor mu?
    pub admission_open: bool,
    /// Olcumun alindigi an.
    pub ts: Timestamp,
}

impl ResourceGauge {
    /// Verilen anda sifir degerli olcum.
    #[must_use]
    pub fn at(ts: Timestamp) -> Self {
        Self {
            rss_kb: 0,
            rss_limit_kb: None,
            cpu_pct: 0.0,
            open_fds: 0,
            fd_limit: None,
            active: 0,
            queued: 0,
            sleeping: 0,
            existing: 0,
            admission_open: true,
            ts,
        }
    }

    /// Var-olan ajan toplami (aktif != var-olan, 3.1).
    #[must_use]
    pub fn total_agents(&self) -> u64 {
        u64::from(self.active) + u64::from(self.queued) + u64::from(self.sleeping)
            + u64::from(self.existing)
    }

    /// Tavan tanimliysa bellek doluluk orani.
    #[must_use]
    pub fn rss_ratio(&self) -> Option<f32> {
        self.rss_limit_kb
            .filter(|tavan| *tavan > 0)
            .map(|tavan| self.rss_kb as f32 / tavan as f32)
    }
}

impl Default for ResourceGauge {
    fn default() -> Self {
        Self::at(crate::now())
    }
}

/// Diff akisi kaydi (`file_touches`, 0008 — 5.2 gorunurluk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTouch {
    /// `file_touches.id`. Henuz yazilmamis kayitta `None`.
    pub id: Option<FileTouchId>,
    /// `file_touches.agent_id`.
    pub agent_id: AgentId,
    /// `file_touches.path`.
    pub path: String,
    /// `file_touches.outside_workspace` — calisma alani disina dokunuldu mu?
    pub outside_workspace: bool,
    /// `file_touches.added` — eklenen satir sayisi.
    pub added: u32,
    /// `file_touches.removed` — silinen satir sayisi.
    pub removed: u32,
    /// `file_touches.pre_ref` — degisim oncesi icerik CAS atifi.
    pub pre_ref: Option<String>,
    /// `file_touches.post_ref` — degisim sonrasi icerik CAS atifi.
    pub post_ref: Option<String>,
    /// `file_touches.ts`.
    pub ts: Timestamp,
}

impl FileTouch {
    /// Net satir degisimi.
    #[must_use]
    pub fn net_lines(&self) -> i64 {
        i64::from(self.added) - i64::from(self.removed)
    }

    /// Yeni dosya olusturuldu mu (oncesi yok, sonrasi var)?
    #[must_use]
    pub fn is_creation(&self) -> bool {
        self.pre_ref.is_none() && self.post_ref.is_some()
    }
}

/// Tool cagrisi goruntusu (`tool_calls`, 0004 — K3 broker denetimi).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallView {
    /// `tool_calls.id`. Henuz yazilmamis kayitta `None`.
    pub id: Option<ToolCallId>,
    /// `tool_calls.agent_id`.
    pub agent_id: AgentId,
    /// `tool_calls.tool`.
    pub tool: String,
    /// `tool_calls.args_json` — cozulmus JSON govdesi.
    pub args: Option<serde_json::Value>,
    /// `tool_calls.result_ref` — cikti CAS atifi (govde DB'de tutulmaz).
    pub result_ref: Option<String>,
    /// `tool_calls.status`.
    pub status: String,
    /// `tool_calls.capability_ok` — yetki broker'i gecti mi?
    pub capability_ok: Option<bool>,
    /// `tool_calls.ts`.
    pub ts: Timestamp,
}

impl ToolCallView {
    /// Broker acikca reddetmis mi?
    #[must_use]
    pub fn is_denied(&self) -> bool {
        self.capability_ok == Some(false)
    }
}

/// Mudahale kaydi (`interrupts`, 0005 — AS2 / 6.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptView {
    /// `interrupts.id`. Henuz yazilmamis kayitta `None`.
    pub id: Option<InterruptId>,
    /// `interrupts.agent_id`.
    pub agent_id: AgentId,
    /// `interrupts.kind`.
    pub kind: String,
    /// `interrupts.source` — hangi yuzden/kanaldan geldigi.
    pub source: String,
    /// Serbest metin gerekce; DB'de tutulmaz, akista tasinir.
    pub reason: Option<String>,
    /// `interrupts.ts`.
    pub ts: Timestamp,
    /// `interrupts.resolved_at`.
    pub resolved_at: Option<Timestamp>,
}

impl InterruptView {
    /// Hala acik mi?
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.resolved_at.is_none()
    }
}

/// Kullaniciya/UI'ya gosterilecek bildirim. DB tablosu yoktur; akis uzerinde
/// tasinir ve `omni-notify` esigi bunun `level` alanina bakar (Bolum 13).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoticeView {
    /// Onem seviyesi.
    pub level: NoticeLevel,
    /// Makine tarafinda eslenebilir kisa kod (dedup penceresi bunu kullanir).
    pub code: String,
    /// Insan okuyacagi metin.
    pub message: String,
    /// Ilgili ajan, varsa.
    pub agent_id: Option<AgentId>,
    /// Ilgili gorev, varsa.
    pub task_id: Option<TaskId>,
    /// Bildirim ani.
    pub ts: Timestamp,
}

impl NoticeView {
    /// Verilen seviyede bildirim uretir.
    #[must_use]
    pub fn new(
        level: NoticeLevel,
        code: impl Into<String>,
        message: impl Into<String>,
        ts: Timestamp,
    ) -> Self {
        Self {
            level,
            code: code.into(),
            message: message.into(),
            agent_id: None,
            task_id: None,
            ts,
        }
    }

    /// Ajan baglamini ekler.
    #[must_use]
    pub fn with_agent(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    /// Gorev baglamini ekler.
    #[must_use]
    pub fn with_task(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }
}

/// Bildirim onem seviyesi. Dis kanallar (Telegram/Twilio) yalnizca yuksek
/// esikte tetiklenir (Bolum 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    /// Bilgilendirme.
    Info,
    /// Dikkat gerektiren ama akisi durdurmayan durum.
    Warn,
    /// Hata.
    Error,
    /// Insan mudahalesi gereken kritik durum.
    Critical,
}

impl NoticeLevel {
    /// Dis kanala (bildirim) tasinmali mi?
    #[must_use]
    pub fn notifies_externally(self) -> bool {
        matches!(self, Self::Error | Self::Critical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> Timestamp {
        crate::now()
    }

    #[test]
    fn tier_db_gidis_donus() {
        for tier in [
            AgentTier::Existing,
            AgentTier::Sleeping,
            AgentTier::Queued,
            AgentTier::Active,
        ] {
            assert_eq!(AgentTier::from_db_str(tier.as_db_str()), Some(tier));
        }
        assert_eq!(AgentTier::from_db_str("yok"), None);
    }

    #[test]
    fn state_db_gidis_donus() {
        for state in [
            AgentState::Init,
            AgentState::Planning,
            AgentState::AwaitingModel,
            AgentState::RunningTool,
            AgentState::Interrupted,
            AgentState::Blocked,
            AgentState::Done,
            AgentState::Failed,
        ] {
            assert_eq!(AgentState::from_db_str(state.as_db_str()), Some(state));
        }
        assert!(AgentState::Done.is_terminal());
        assert!(!AgentState::Planning.is_terminal());
    }

    #[test]
    fn health_db_gidis_donus() {
        for h in [
            ProviderHealthState::Healthy,
            ProviderHealthState::Degraded,
            ProviderHealthState::Down,
            ProviderHealthState::QuotaExhausted,
        ] {
            assert_eq!(ProviderHealthState::from_db_str(h.as_db_str()), Some(h));
        }
        assert!(!ProviderHealthState::QuotaExhausted.is_usable());
    }

    #[test]
    fn bos_snapshot_tutarli() {
        let snap = SystemSnapshot::empty(ts());
        assert_eq!(snap.active_count(), 0);
        assert!(snap.agent(1).is_none());
        assert!(snap.task(1).is_none());
        assert_eq!(snap.resource.total_agents(), 0);
    }

    #[test]
    fn butce_kalani_hesaplanir() {
        let mut task = TaskView {
            id: 1,
            parent_id: None,
            root_id: 1,
            title: "kok".into(),
            mode: "autonomous".into(),
            status: "running".into(),
            depth: 0,
            budget_allocated: Some(10.0),
            budget_spent: Some(2.5),
            duration_target: Some("mvp".into()),
            created_at: ts(),
            closed_at: None,
        };
        assert!(task.is_root());
        assert_eq!(task.budget_remaining(), Some(7.5));
        task.budget_allocated = None;
        assert_eq!(task.budget_remaining(), None);
    }

    #[test]
    fn dosya_dokunusu_net_satir() {
        let touch = FileTouch {
            id: None,
            agent_id: 7,
            path: "crates/omni/omni-proto/src/lib.rs".into(),
            outside_workspace: false,
            added: 12,
            removed: 3,
            pre_ref: None,
            post_ref: Some("blake3:abc".into()),
            ts: ts(),
        };
        assert_eq!(touch.net_lines(), 9);
        assert!(touch.is_creation());
    }

    #[test]
    fn bildirim_esigi() {
        assert!(NoticeLevel::Critical.notifies_externally());
        assert!(!NoticeLevel::Info.notifies_externally());
        assert!(NoticeLevel::Info < NoticeLevel::Critical);
        let n = NoticeView::new(NoticeLevel::Warn, "depth_cap", "derinlik tavani", ts())
            .with_agent(3)
            .with_task(1);
        assert_eq!(n.agent_id, Some(3));
        assert_eq!(n.task_id, Some(1));
    }
}
