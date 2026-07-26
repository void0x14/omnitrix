//! Durum olay akisi (MASTER-PLAN 6.1 / 6.2 okuma sozlesmesi).
//!
//! TUI ve WebUI **ayni** `StateEvent` akisini tuketir; yalnizca render farklidir
//! (K7). Tel formati JSON: ic-etiketli (`"type"`) ki iki yuz de ayni ayristirmayi
//! kullansin.

use serde::{Deserialize, Serialize};

use crate::state::{
    AgentView, FileTouch, InterruptView, NoticeView, ResourceGauge, TaskView, ToolCallView,
};
use crate::{AgentId, EventSeq, ProtoError, Timestamp};

/// Cekirdekten UI'a akan tek durum degisimi.
///
/// `*Upserted` olaylari **idempotent**tir: UI ilgili satiri yoksa ekler, varsa
/// degistirir. Boylece akista bosluk olsa bile yeniden `SystemSnapshot` cekmek
/// dogru sonucu verir (crash-only, Bolum 8).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateEvent {
    /// Ajan olusturuldu ya da alanlari degisti.
    AgentUpserted(AgentView),
    /// Gorev olusturuldu ya da alanlari degisti.
    TaskUpserted(TaskView),
    /// Bir dosyaya dokunuldu (5.2 diff akisi).
    FileTouched(FileTouch),
    /// Tool cagrisi kaydedildi ya da durumu degisti.
    ToolCall(ToolCallView),
    /// Mudahale acildi ya da cozuldu.
    Interrupt(InterruptView),
    /// Bildirim.
    Notice(NoticeView),
    /// Kaynak valisi olcumu (7.2).
    ResourceTick(ResourceGauge),
}

impl StateEvent {
    /// Olayin ait oldugu ajan, belirlenebiliyorsa.
    #[must_use]
    pub fn agent_id(&self) -> Option<AgentId> {
        match self {
            Self::AgentUpserted(a) => Some(a.id),
            Self::FileTouched(f) => Some(f.agent_id),
            Self::ToolCall(t) => Some(t.agent_id),
            Self::Interrupt(i) => Some(i.agent_id),
            Self::Notice(n) => n.agent_id,
            Self::TaskUpserted(_) | Self::ResourceTick(_) => None,
        }
    }

    /// Olayin zaman damgasi.
    #[must_use]
    pub fn ts(&self) -> Timestamp {
        match self {
            Self::AgentUpserted(_) | Self::TaskUpserted(_) => self.fallback_ts(),
            Self::FileTouched(f) => f.ts,
            Self::ToolCall(t) => t.ts,
            Self::Interrupt(i) => i.ts,
            Self::Notice(n) => n.ts,
            Self::ResourceTick(r) => r.ts,
        }
    }

    /// `AgentView`/`TaskView` kendi zaman damgasini tasimadigi icin gorev
    /// olusturma ani ya da simdiki zaman kullanilir.
    fn fallback_ts(&self) -> Timestamp {
        match self {
            Self::TaskUpserted(t) => t.created_at,
            _ => crate::now(),
        }
    }

    /// SSE `event:` alanina yazilacak kanonik ad.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AgentUpserted(_) => "agent_upserted",
            Self::TaskUpserted(_) => "task_upserted",
            Self::FileTouched(_) => "file_touched",
            Self::ToolCall(_) => "tool_call",
            Self::Interrupt(_) => "interrupt",
            Self::Notice(_) => "notice",
            Self::ResourceTick(_) => "resource_tick",
        }
    }

    /// SSE/WS govdesine cevirir.
    ///
    /// # Errors
    /// Serilestirme basarisiz olursa [`ProtoError::Json`] doner.
    pub fn to_json(&self) -> Result<String, ProtoError> {
        crate::to_json(self)
    }

    /// SSE/WS govdesinden cozer.
    ///
    /// # Errors
    /// Govde beklenen sekilde degilse [`ProtoError::Json`] doner.
    pub fn from_json(raw: &str) -> Result<Self, ProtoError> {
        crate::from_json(raw)
    }
}

/// Akista tasinan olay zarfi: global sira numarasi + olay.
///
/// UI kopan baglantidan sonra `since = last_seq` ile devam eder; boslugu
/// gorurse `SystemSnapshot`'i yeniden ceker (Bolum 6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateFrame {
    /// Control-plane'in urettigi monoton artan sira numarasi.
    pub seq: EventSeq,
    /// Zarfin uretildigi an.
    pub ts: Timestamp,
    /// Tasinan olay.
    pub event: StateEvent,
}

impl StateFrame {
    /// Verilen sira numarasiyla zarf uretir.
    #[must_use]
    pub fn new(seq: EventSeq, event: StateEvent) -> Self {
        let ts = event.ts();
        Self { seq, ts, event }
    }

    /// Bu zarfin beklenen bir sonraki sirayi izleyip izlemedigi.
    #[must_use]
    pub fn follows(&self, previous_seq: EventSeq) -> bool {
        self.seq == previous_seq.saturating_add(1)
    }

    /// SSE/WS govdesine cevirir.
    ///
    /// # Errors
    /// Serilestirme basarisiz olursa [`ProtoError::Json`] doner.
    pub fn to_json(&self) -> Result<String, ProtoError> {
        crate::to_json(self)
    }

    /// SSE/WS govdesinden cozer.
    ///
    /// # Errors
    /// Govde beklenen sekilde degilse [`ProtoError::Json`] doner.
    pub fn from_json(raw: &str) -> Result<Self, ProtoError> {
        crate::from_json(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentState, AgentTier, NoticeLevel};

    fn ornek_ajan() -> AgentView {
        AgentView {
            id: 42,
            persona: "planner".into(),
            tier: AgentTier::Active,
            task_id: 1,
            parent_id: None,
            state: AgentState::Planning,
            rss_kb: 2048,
            tokens_in: 100,
            tokens_out: 50,
            cost: 0.01,
            trust: 1.0,
            depth: 0,
            last_event_seq: 7,
        }
    }

    #[test]
    fn olay_json_gidis_donus() {
        let ev = StateEvent::AgentUpserted(ornek_ajan());
        let raw = ev.to_json().expect("kodlama");
        assert!(raw.contains("\"type\":\"agent_upserted\""));
        let geri = StateEvent::from_json(&raw).expect("cozme");
        assert_eq!(geri.agent_id(), Some(42));
        assert_eq!(geri.kind(), "agent_upserted");
    }

    #[test]
    fn kaynak_olayinin_ajani_yok() {
        let ev = StateEvent::ResourceTick(ResourceGauge::default());
        assert_eq!(ev.agent_id(), None);
        assert_eq!(ev.kind(), "resource_tick");
    }

    #[test]
    fn bildirim_olayi_ajan_tasir() {
        let notice = NoticeView::new(NoticeLevel::Error, "spawn_denied", "fan-out tavani", crate::now())
            .with_agent(9);
        let ev = StateEvent::Notice(notice);
        assert_eq!(ev.agent_id(), Some(9));
    }

    #[test]
    fn zarf_sira_takibi() {
        let frame = StateFrame::new(5, StateEvent::AgentUpserted(ornek_ajan()));
        assert!(frame.follows(4));
        assert!(!frame.follows(3));
        let raw = frame.to_json().expect("kodlama");
        let geri = StateFrame::from_json(&raw).expect("cozme");
        assert_eq!(geri.seq, 5);
    }
}
