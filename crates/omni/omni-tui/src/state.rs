//! UI durum indirgeyicisi (MASTER-PLAN 6.2 okuma sozlesmesi).
//!
//! Bu modul **kendi durum tipini icat etmez** (I3). Tasidigi her sey
//! `omni-proto` kanonik tipidir; burada yalnizca *turetilmis gorunum* durur:
//! ilk yuklemedeki [`SystemSnapshot`] uzerine [`StateEvent`] akisi uygulanir ve
//! render'in ihtiyac duydugu birkac toplam (ajan basina `+`/`-` diff, son tool
//! cagrilari, acik mudahaleler) tutulur.
//!
//! Ayni akis WebUI'ya da gider; farkli olan yalnizca render'dir (K7).

use std::collections::{BTreeMap, VecDeque};

use omni_proto::{
    AgentId, AgentView, EventSeq, FileTouch, InterruptView, NoticeView, ProviderView,
    ResourceGauge, StateEvent, StateFrame, SystemSnapshot, TaskView, ToolCallView,
};

/// Bellekte tutulan en fazla tool cagrisi (halka tampon).
pub const MAX_TOOL_CALLS: usize = 256;

/// Bellekte tutulan en fazla bildirim.
pub const MAX_NOTICES: usize = 128;

/// Bellekte tutulan en fazla mudahale kaydi.
pub const MAX_INTERRUPTS: usize = 64;

/// Bellekte tutulan en fazla dosya dokunusu (global diff akisi, 9.3).
pub const MAX_TOUCHES: usize = 512;

/// Onay bekleyen tool cagrisinin `tool_calls.status` degeri.
pub const STATUS_PENDING: &str = "pending";

/// Bir olay zarfinin uygulanma sonucu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Zarf sirasi takip ediyordu, uygulandi.
    Applied,
    /// Sira atlandi: olay yine de uygulandi (upsert'ler idempotent) ama UI
    /// tutarlilik icin `SystemSnapshot`'i yeniden cekmelidir (Bolum 6.2).
    Gap,
    /// Zarf zaten gorulmus bir siradan geliyor; yok sayildi.
    Stale,
}

impl ApplyOutcome {
    /// Snapshot yeniden cekilmeli mi?
    pub fn needs_resync(self) -> bool {
        matches!(self, Self::Gap)
    }
}

/// Tek bir dosyanin biriken diff istatistigi (9.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiffStat {
    /// Eklenen satir toplami.
    pub added: u64,
    /// Silinen satir toplami.
    pub removed: u64,
    /// Bu dosyaya kac kez dokunuldu.
    pub touches: u32,
    /// Calisma alani disinda mi (K5 + 5.2 gorunurluk).
    pub outside_workspace: bool,
}

impl FileDiffStat {
    /// Net satir degisimi.
    pub fn net(&self) -> i64 {
        i64::try_from(self.added).unwrap_or(i64::MAX)
            - i64::try_from(self.removed).unwrap_or(i64::MAX)
    }

    /// Grafik olceginde kullanilan toplam hacim.
    pub fn volume(&self) -> u64 {
        self.added.saturating_add(self.removed)
    }
}

/// Bir ajanin dokundugu tum dosyalarin toplami (9.3 — "kayip opencode ozelligi").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentDiffStat {
    /// Tum dosyalarda eklenen satir toplami.
    pub added: u64,
    /// Tum dosyalarda silinen satir toplami.
    pub removed: u64,
    /// Calisma alani disina dusen dosya sayisi.
    pub outside_workspace: u32,
    files: BTreeMap<String, FileDiffStat>,
}

impl AgentDiffStat {
    /// Bir dosya dokunusunu toplamlara isler.
    pub fn record(&mut self, touch: &FileTouch) {
        self.added = self.added.saturating_add(u64::from(touch.added));
        self.removed = self.removed.saturating_add(u64::from(touch.removed));

        let slot = self.files.entry(touch.path.clone()).or_default();
        let disari_yeni = touch.outside_workspace && !slot.outside_workspace;
        slot.added = slot.added.saturating_add(u64::from(touch.added));
        slot.removed = slot.removed.saturating_add(u64::from(touch.removed));
        slot.touches = slot.touches.saturating_add(1);
        slot.outside_workspace |= touch.outside_workspace;
        if disari_yeni {
            self.outside_workspace = self.outside_workspace.saturating_add(1);
        }
    }

    /// Dokunulan dosya sayisi.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Dosya istatistigini yola gore arar.
    pub fn file(&self, path: &str) -> Option<&FileDiffStat> {
        self.files.get(path)
    }

    /// Dosyalari hacme gore azalan sirada verir (esitlikte yol adina gore).
    pub fn ranked(&self) -> Vec<(&str, &FileDiffStat)> {
        let mut satirlar: Vec<(&str, &FileDiffStat)> = self
            .files
            .iter()
            .map(|(yol, stat)| (yol.as_str(), stat))
            .collect();
        satirlar.sort_by(|sol, sag| {
            sag.1
                .volume()
                .cmp(&sol.1.volume())
                .then_with(|| sol.0.cmp(sag.0))
        });
        satirlar
    }

    /// Net satir degisimi.
    pub fn net(&self) -> i64 {
        i64::try_from(self.added).unwrap_or(i64::MAX)
            - i64::try_from(self.removed).unwrap_or(i64::MAX)
    }

    /// Grafik olceginde kullanilan toplam hacim.
    pub fn volume(&self) -> u64 {
        self.added.saturating_add(self.removed)
    }

    /// Hic dokunus islenmemis mi?
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// TUI'nin okudugu turetilmis durum. Kanonik alanlar `SystemSnapshot` icinde
/// durur; disari yalnizca okuma erisimi verilir ki yuz kendi kopyasini
/// tutmasin (I3).
#[derive(Debug, Clone)]
pub struct UiState {
    snapshot: SystemSnapshot,
    last_seq: EventSeq,
    resync_needed: bool,
    diffs: BTreeMap<AgentId, AgentDiffStat>,
    touches: VecDeque<FileTouch>,
    tool_calls: VecDeque<ToolCallView>,
    interrupts: VecDeque<InterruptView>,
    notices: VecDeque<NoticeView>,
    status: String,
    local_rss_kb: u64,
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

impl UiState {
    /// Bos durum (cold-start; henuz snapshot cekilmemis).
    pub fn new() -> Self {
        Self {
            snapshot: SystemSnapshot::empty(omni_proto::now()),
            last_seq: 0,
            resync_needed: false,
            diffs: BTreeMap::new(),
            touches: VecDeque::new(),
            tool_calls: VecDeque::new(),
            interrupts: VecDeque::new(),
            notices: VecDeque::new(),
            status: String::new(),
            local_rss_kb: 0,
        }
    }

    /// Ilk yukleme: `omni-control`'den gelen `SystemSnapshot`'i yerlestirir.
    ///
    /// Dosya dokunusu toplamlari snapshot'ta tasinmadigi icin korunur; akis
    /// bosluklari `last_seq` sifirlanarak kapatilir.
    pub fn load(&mut self, snapshot: SystemSnapshot) {
        self.snapshot = snapshot;
        self.snapshot.agents.sort_by_key(|a| a.id);
        self.snapshot.tasks.sort_by_key(|t| t.id);
        self.snapshot.providers.sort_by_key(|p| p.id);
        self.last_seq = 0;
        self.resync_needed = false;
    }

    /// Akistan gelen zarfi uygular ve sira butunlugunu raporlar.
    pub fn apply(&mut self, frame: StateFrame) -> ApplyOutcome {
        if self.last_seq != 0 && frame.seq <= self.last_seq {
            return ApplyOutcome::Stale;
        }
        let bosluk = self.last_seq != 0 && !frame.follows(self.last_seq);
        self.last_seq = frame.seq;
        self.apply_event(frame.event);
        if bosluk {
            self.resync_needed = true;
            tracing::warn!(
                seq = frame.seq,
                "olay akisinda bosluk; snapshot tazelenmeli"
            );
            ApplyOutcome::Gap
        } else {
            ApplyOutcome::Applied
        }
    }

    /// Zarfsiz olay uygular (yerel uretim ya da test).
    pub fn apply_event(&mut self, event: StateEvent) {
        match event {
            StateEvent::AgentUpserted(view) => upsert_agent(&mut self.snapshot.agents, view),
            StateEvent::TaskUpserted(view) => upsert_task(&mut self.snapshot.tasks, view),
            StateEvent::FileTouched(touch) => {
                self.diffs.entry(touch.agent_id).or_default().record(&touch);
                push_bounded(&mut self.touches, touch, MAX_TOUCHES);
            }
            StateEvent::ToolCall(call) => {
                if let Some(id) = call.id
                    && let Some(slot) = self
                        .tool_calls
                        .iter_mut()
                        .find(|mevcut| mevcut.id == Some(id))
                {
                    *slot = call;
                    return;
                }
                push_bounded(&mut self.tool_calls, call, MAX_TOOL_CALLS);
            }
            StateEvent::Interrupt(kayit) => {
                if let Some(id) = kayit.id
                    && let Some(slot) = self
                        .interrupts
                        .iter_mut()
                        .find(|mevcut| mevcut.id == Some(id))
                {
                    *slot = kayit;
                    return;
                }
                push_bounded(&mut self.interrupts, kayit, MAX_INTERRUPTS);
            }
            StateEvent::Notice(notice) => push_bounded(&mut self.notices, notice, MAX_NOTICES),
            StateEvent::ResourceTick(gauge) => self.snapshot.resource = gauge,
        }
    }

    /// Kanonik goruntuye salt-okunur erisim.
    pub fn snapshot(&self) -> &SystemSnapshot {
        &self.snapshot
    }

    /// Ajanlar (kimlige gore sirali).
    pub fn agents(&self) -> &[AgentView] {
        &self.snapshot.agents
    }

    /// Gorevler (kimlige gore sirali).
    pub fn tasks(&self) -> &[TaskView] {
        &self.snapshot.tasks
    }

    /// Saglayicilar.
    pub fn providers(&self) -> &[ProviderView] {
        &self.snapshot.providers
    }

    /// Son kaynak valisi olcumu (7.2).
    pub fn resource(&self) -> &ResourceGauge {
        &self.snapshot.resource
    }

    /// Kimlige gore ajan.
    pub fn agent(&self, id: AgentId) -> Option<&AgentView> {
        self.snapshot.agent(id)
    }

    /// Kimlige gore gorev.
    pub fn task(&self, id: omni_proto::TaskId) -> Option<&TaskView> {
        self.snapshot.task(id)
    }

    /// Ajanin diff toplami (9.3).
    pub fn diff(&self, agent_id: AgentId) -> Option<&AgentDiffStat> {
        self.diffs.get(&agent_id)
    }

    /// Tum ajanlarin diff toplamlari.
    pub fn diffs(&self) -> &BTreeMap<AgentId, AgentDiffStat> {
        &self.diffs
    }

    /// Global diff akisi (en yeni sonda).
    pub fn touches(&self) -> impl DoubleEndedIterator<Item = &FileTouch> {
        self.touches.iter()
    }

    /// Son tool cagrilari (en yeni sonda).
    pub fn tool_calls(&self) -> impl DoubleEndedIterator<Item = &ToolCallView> {
        self.tool_calls.iter()
    }

    /// Verilen ajanin tool cagrilari (en yeni once).
    pub fn tool_calls_for(&self, agent_id: AgentId) -> Vec<&ToolCallView> {
        self.tool_calls
            .iter()
            .rev()
            .filter(|call| call.agent_id == agent_id)
            .collect()
    }

    /// Henuz cozulmemis mudahaleler.
    pub fn open_interrupts(&self) -> Vec<&InterruptView> {
        self.interrupts.iter().filter(|i| i.is_open()).collect()
    }

    /// Bildirimler (en yeni sonda).
    pub fn notices(&self) -> impl DoubleEndedIterator<Item = &NoticeView> {
        self.notices.iter()
    }

    /// Insan onayi bekleyen tool cagrilari (K3 yetki broker'i).
    pub fn pending_approvals(&self) -> Vec<&ToolCallView> {
        self.tool_calls
            .iter()
            .rev()
            .filter(|call| call.status == STATUS_PENDING && call.capability_ok.is_none())
            .collect()
    }

    /// Verilen ajan icin onay bekleyen ilk tool cagrisi.
    pub fn first_pending_approval(&self, agent_id: AgentId) -> Option<&ToolCallView> {
        self.tool_calls.iter().rev().find(|call| {
            call.agent_id == agent_id
                && call.status == STATUS_PENDING
                && call.capability_ok.is_none()
        })
    }

    /// Gorulen son akis sirasi.
    pub fn last_seq(&self) -> EventSeq {
        self.last_seq
    }

    /// Akista bosluk gorulduse `true`; snapshot tazelendiginde temizlenir.
    pub fn needs_resync(&self) -> bool {
        self.resync_needed
    }

    /// Snapshot tazeleme ihtiyacini disaridan isaretler.
    ///
    /// Kontrol duzlemi yayin tamponu tastiginda sira numarasi degil, bir
    /// `lagged` cercevesi gonderir (Bolum 6.2); akis surucusu boyle bir
    /// durumda bunu cagirir. [`UiState::load`] bayragi temizler.
    pub fn mark_resync(&mut self) {
        self.resync_needed = true;
    }

    /// Yerel durum satiri (isinma fazi vb.). Kanonik durumun parcasi degildir;
    /// yalnizca bu yuzun kendi surecine dair bilgi tasir.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Yerel durum satirini degistirir.
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Bu surecin olculen RSS degeri (kB). Cekirdek olcum gelmeden once
    /// gosterilecek yerel deger.
    pub fn local_rss_kb(&self) -> u64 {
        self.local_rss_kb
    }

    /// Yerel RSS olcumunu degistirir.
    pub fn set_local_rss_kb(&mut self, kb: u64) {
        self.local_rss_kb = kb;
    }
}

/// Kimlige gore sirali listeye idempotent ekleme/guncelleme.
fn upsert_agent(list: &mut Vec<AgentView>, view: AgentView) {
    match list.binary_search_by_key(&view.id, |a| a.id) {
        Ok(idx) => {
            if let Some(slot) = list.get_mut(idx) {
                *slot = view;
            }
        }
        Err(idx) => list.insert(idx, view),
    }
}

/// Kimlige gore sirali listeye idempotent ekleme/guncelleme.
fn upsert_task(list: &mut Vec<TaskView>, view: TaskView) {
    match list.binary_search_by_key(&view.id, |t| t.id) {
        Ok(idx) => {
            if let Some(slot) = list.get_mut(idx) {
                *slot = view;
            }
        }
        Err(idx) => list.insert(idx, view),
    }
}

/// Halka tampona ekler; tavani asarsa en eskiyi dusurur.
fn push_bounded<T>(ring: &mut VecDeque<T>, item: T, cap: usize) {
    if ring.len() >= cap {
        ring.pop_front();
    }
    ring.push_back(item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{AgentState, AgentTier, NoticeLevel};

    fn ajan(id: AgentId) -> AgentView {
        AgentView {
            id,
            persona: "planner".into(),
            tier: AgentTier::Active,
            task_id: 1,
            parent_id: None,
            state: AgentState::Planning,
            rss_kb: 1024,
            tokens_in: 10,
            tokens_out: 5,
            cost: 0.001,
            trust: 1.0,
            depth: 0,
            last_event_seq: 0,
        }
    }

    fn dokunus(agent_id: AgentId, path: &str, added: u32, removed: u32) -> FileTouch {
        FileTouch {
            id: None,
            agent_id,
            path: path.into(),
            outside_workspace: false,
            added,
            removed,
            pre_ref: None,
            post_ref: Some("blake3:x".into()),
            ts: omni_proto::now(),
        }
    }

    fn cagri(id: i64, agent_id: AgentId, status: &str) -> ToolCallView {
        ToolCallView {
            id: Some(id),
            agent_id,
            tool: "write".into(),
            args: Some(serde_json::json!({ "path": "a.rs" })),
            result_ref: None,
            status: status.into(),
            capability_ok: None,
            ts: omni_proto::now(),
        }
    }

    #[test]
    fn ajan_upsert_idempotenttir() {
        let mut state = UiState::new();
        state.apply_event(StateEvent::AgentUpserted(ajan(2)));
        state.apply_event(StateEvent::AgentUpserted(ajan(1)));
        assert_eq!(state.agents().len(), 2);
        assert_eq!(state.agents()[0].id, 1);

        let mut guncel = ajan(1);
        guncel.state = AgentState::Done;
        state.apply_event(StateEvent::AgentUpserted(guncel));
        assert_eq!(state.agents().len(), 2);
        assert_eq!(state.agent(1).map(|a| a.state), Some(AgentState::Done));
    }

    #[test]
    fn dosya_dokunuslari_ajan_basina_toplanir() {
        let mut state = UiState::new();
        state.apply_event(StateEvent::FileTouched(dokunus(7, "a.rs", 10, 2)));
        state.apply_event(StateEvent::FileTouched(dokunus(7, "a.rs", 3, 1)));
        state.apply_event(StateEvent::FileTouched(dokunus(7, "b.rs", 1, 0)));

        let stat = state.diff(7).expect("diff");
        assert_eq!(stat.added, 14);
        assert_eq!(stat.removed, 3);
        assert_eq!(stat.file_count(), 2);
        assert_eq!(stat.net(), 11);
        let siralama = stat.ranked();
        assert_eq!(siralama[0].0, "a.rs");
        assert_eq!(siralama[0].1.touches, 2);
    }

    #[test]
    fn calisma_alani_disi_dokunuslar_sayilir() {
        let mut state = UiState::new();
        let mut disari = dokunus(3, "/etc/hosts", 1, 1);
        disari.outside_workspace = true;
        state.apply_event(StateEvent::FileTouched(disari.clone()));
        state.apply_event(StateEvent::FileTouched(disari));
        let stat = state.diff(3).expect("diff");
        assert_eq!(stat.outside_workspace, 1);
        assert_eq!(stat.file_count(), 1);
    }

    #[test]
    fn sira_boslugu_resync_ister() {
        let mut state = UiState::new();
        assert_eq!(
            state.apply(StateFrame::new(1, StateEvent::AgentUpserted(ajan(1)))),
            ApplyOutcome::Applied
        );
        assert_eq!(
            state.apply(StateFrame::new(2, StateEvent::AgentUpserted(ajan(2)))),
            ApplyOutcome::Applied
        );
        assert!(!state.needs_resync());

        let sonuc = state.apply(StateFrame::new(9, StateEvent::AgentUpserted(ajan(3))));
        assert_eq!(sonuc, ApplyOutcome::Gap);
        assert!(sonuc.needs_resync());
        assert!(state.needs_resync());
        // Bosluga ragmen olay uygulanir (upsert idempotent).
        assert!(state.agent(3).is_some());
    }

    #[test]
    fn eski_zarf_yok_sayilir() {
        let mut state = UiState::new();
        let _ = state.apply(StateFrame::new(5, StateEvent::AgentUpserted(ajan(1))));
        let sonuc = state.apply(StateFrame::new(4, StateEvent::AgentUpserted(ajan(2))));
        assert_eq!(sonuc, ApplyOutcome::Stale);
        assert!(state.agent(2).is_none());
        assert_eq!(state.last_seq(), 5);
    }

    #[test]
    fn tool_cagrisi_kimlige_gore_guncellenir() {
        let mut state = UiState::new();
        state.apply_event(StateEvent::ToolCall(cagri(1, 4, STATUS_PENDING)));
        assert_eq!(state.pending_approvals().len(), 1);
        assert!(state.first_pending_approval(4).is_some());

        let mut kapali = cagri(1, 4, "ok");
        kapali.capability_ok = Some(true);
        state.apply_event(StateEvent::ToolCall(kapali));
        assert_eq!(state.tool_calls().count(), 1);
        assert!(state.pending_approvals().is_empty());
    }

    #[test]
    fn bildirim_halkasi_tavani_asmaz() {
        let mut state = UiState::new();
        for i in 0..(MAX_NOTICES + 10) {
            state.apply_event(StateEvent::Notice(NoticeView::new(
                NoticeLevel::Info,
                "test",
                format!("mesaj {i}"),
                omni_proto::now(),
            )));
        }
        assert_eq!(state.notices().count(), MAX_NOTICES);
    }

    #[test]
    fn snapshot_yuklemesi_diff_toplamlarini_korur() {
        let mut state = UiState::new();
        state.apply_event(StateEvent::FileTouched(dokunus(1, "a.rs", 5, 0)));
        let _ = state.apply(StateFrame::new(3, StateEvent::AgentUpserted(ajan(1))));

        let mut snap = SystemSnapshot::empty(omni_proto::now());
        snap.agents.push(ajan(2));
        snap.agents.push(ajan(1));
        state.load(snap);

        assert_eq!(state.agents()[0].id, 1);
        assert_eq!(state.last_seq(), 0);
        assert!(!state.needs_resync());
        assert_eq!(state.diff(1).map(AgentDiffStat::volume), Some(5));
    }
}
