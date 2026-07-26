//! Merkezi durum deposu — **tek yazar** (MASTER-PLAN 6.2).
//!
//! Sistemdeki her durum mutasyonu bu dosyadaki [`CoreState`] uzerinden gecer.
//! Baska hicbir katman `AgentView`/`TaskView` alanlarini dogrudan degistirmez;
//! alt katmanlar (router, tools, scheduler) [`CoreState::ingest`] ile olay
//! bildirir, UI [`CoreState::apply`] ile komut gonderir. Cikan `StateEvent`
//! listesi `omni-control`'e verilir, o da her iki yuze yayar (K7) ve
//! `omni-storage` writer-actor'e WAL yazar (I7).
//!
//! Bu dosyada I/O yoktur: `CoreState` saf, senkron ve deterministiktir.
//! Kalicilik cagiranin sorumlulugudur — boylece tek yazar kurali kilitsiz
//! korunur.

use std::collections::BTreeMap;

use omni_proto::{
    AgentId, AgentState, AgentTier, AgentView, Command, EventSeq, InterruptView, NoticeLevel,
    NoticeView, ProviderId, ProviderView, ResourceGauge, RoutingStrategy, StateEvent,
    SystemSnapshot, TaskId, TaskView, Timestamp, now,
};

use crate::CoreError;

/// Yeni acilan gorevin baslangic durumu (`tasks.status`).
///
/// Deger serbest metindir (sema `TEXT`); burada yalnizca cekirdegin urettigi
/// baslangic degeri sabitlenir ki iki yuz ayni etiketi gorsun.
pub const TASK_STATUS_OPEN: &str = "open";

/// Persona verilmediginde kok ajana atanan persona dosyasi adi (K12).
///
/// Bu bir MODEL adi degildir; diskteki `config/personas/<ad>.toml` dosyasina
/// karsilik gelir (I5 ihlali yok).
pub const DEFAULT_PERSONA: &str = "default";

/// Yeni ajanin baslangic guven puani (`trust_scores.score`, 3.4).
const INITIAL_TRUST: f32 = 1.0;

/// Ajan durum makinesi (MASTER-PLAN 6.1).
///
/// `Init -> Planning -> AwaitingModel -> RunningTool -> (Interrupted|Blocked)
/// -> (Done|Failed)`. Gecerli olmayan gecis **reddedilir**; sessizce kabul
/// edilmez. `Done`/`Failed` sonlanmis durumlardir, cikisi yoktur.
pub struct AgentStateMachine;

impl AgentStateMachine {
    /// Verilen durumdan gidilebilecek durumlar.
    #[must_use]
    pub fn successors(from: AgentState) -> &'static [AgentState] {
        match from {
            AgentState::Init => &[
                AgentState::Planning,
                AgentState::Blocked,
                AgentState::Interrupted,
                AgentState::Failed,
            ],
            AgentState::Planning => &[
                AgentState::AwaitingModel,
                AgentState::RunningTool,
                AgentState::Blocked,
                AgentState::Interrupted,
                AgentState::Done,
                AgentState::Failed,
            ],
            AgentState::AwaitingModel => &[
                AgentState::Planning,
                AgentState::RunningTool,
                AgentState::Blocked,
                AgentState::Interrupted,
                AgentState::Done,
                AgentState::Failed,
            ],
            AgentState::RunningTool => &[
                AgentState::Planning,
                AgentState::AwaitingModel,
                AgentState::Blocked,
                AgentState::Interrupted,
                AgentState::Done,
                AgentState::Failed,
            ],
            // Mudahale sonrasi ajan plana doner; dogrudan tool calistiramaz.
            AgentState::Interrupted => &[
                AgentState::Planning,
                AgentState::Blocked,
                AgentState::Done,
                AgentState::Failed,
            ],
            // Onay geldiginde bekleyen tool cagrisi kaldigi yerden surer.
            AgentState::Blocked => &[
                AgentState::Planning,
                AgentState::RunningTool,
                AgentState::Interrupted,
                AgentState::Done,
                AgentState::Failed,
            ],
            AgentState::Done | AgentState::Failed => &[],
        }
    }

    /// `from -> to` gecisi gecerli mi? Ayni duruma gecis burada `false`
    /// doner; cagiran tarafta yinelenen atama olay uretmeyen no-op'tur.
    #[must_use]
    pub fn allows(from: AgentState, to: AgentState) -> bool {
        Self::successors(from).contains(&to)
    }
}

/// Yonlendirme ayari (`routing_policies`, 10.1).
///
/// Tel formatinda karsiligi yoktur — `Command::SetRouting` ile gelen son karar
/// cekirdekte burada tutulur, `omni-router` buradan okur. Literal model adi
/// icermez; rol->model eslemesi `config` JSON'undan gelir (AS7/I5).
#[derive(Debug, Clone)]
pub struct RoutingSetting {
    /// `routing_policies.name`.
    pub policy: String,
    /// `routing_policies.strategy`.
    pub strategy: RoutingStrategy,
    /// `routing_policies.config_json`.
    pub config: serde_json::Value,
    /// Ayarin uygulandigi an.
    pub updated_at: Timestamp,
}

/// Cekirdek durum deposu.
///
/// Ajanlar, gorevler, saglayicilar ve kaynak olcumu tek yerde durur.
/// `snapshot()` okuma sozlesmesini, `apply()` yazma sozlesmesini, `ingest()`
/// alt katman geri-bildirimini karsilar (6.2).
#[derive(Debug)]
pub struct CoreState {
    agents: BTreeMap<AgentId, AgentView>,
    tasks: BTreeMap<TaskId, TaskView>,
    providers: BTreeMap<ProviderId, ProviderView>,
    interrupts: Vec<InterruptView>,
    routing: Option<RoutingSetting>,
    resource: ResourceGauge,
    next_agent_id: AgentId,
    next_task_id: TaskId,
    seq: EventSeq,
    default_persona: String,
    depth_cap: Option<u8>,
}

impl Default for CoreState {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreState {
    /// Bos cekirdek. Kimlik sayaclari 1'den baslar; DB'den hidrasyon
    /// [`CoreState::hydrate`] ile yapilir.
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            tasks: BTreeMap::new(),
            providers: BTreeMap::new(),
            interrupts: Vec::new(),
            routing: None,
            resource: ResourceGauge::at(now()),
            next_agent_id: 1,
            next_task_id: 1,
            seq: 0,
            default_persona: DEFAULT_PERSONA.to_owned(),
            depth_cap: None,
        }
    }

    /// Persona verilmeyen kok ajanlar icin varsayilan persona adini baglar.
    #[must_use]
    pub fn with_default_persona(mut self, persona: impl Into<String>) -> Self {
        let persona = persona.into();
        if !persona.trim().is_empty() {
            self.default_persona = persona;
        }
        self
    }

    /// Gorev agacinin derinlik tavanini baglar (7.3/AS3).
    ///
    /// Kod kendiliginden tavan koymaz; tavan yalnizca yapilandirmadan gelirse
    /// uygulanir (K1).
    #[must_use]
    pub fn with_depth_cap(mut self, cap: u8) -> Self {
        self.depth_cap = Some(cap);
        self
    }

    // -- okuma --------------------------------------------------------------

    /// UI'nin ilk yuklemede aldigi tam goruntu (6.2).
    #[must_use]
    pub fn snapshot(&self) -> SystemSnapshot {
        SystemSnapshot {
            agents: self.agents.values().cloned().collect(),
            tasks: self.tasks.values().cloned().collect(),
            providers: self.providers.values().cloned().collect(),
            resource: self.resource.clone(),
            ts: now(),
        }
    }

    /// Tek ajan.
    #[must_use]
    pub fn agent(&self, id: AgentId) -> Option<&AgentView> {
        self.agents.get(&id)
    }

    /// Tek gorev.
    #[must_use]
    pub fn task(&self, id: TaskId) -> Option<&TaskView> {
        self.tasks.get(&id)
    }

    /// Henuz cozulmemis mudahaleler (AS2).
    pub fn open_interrupts(&self) -> impl Iterator<Item = &InterruptView> {
        self.interrupts.iter().filter(|i| i.is_open())
    }

    /// Son yonlendirme ayari.
    #[must_use]
    pub fn routing(&self) -> Option<&RoutingSetting> {
        self.routing.as_ref()
    }

    /// Uretilmis son olay sirasi.
    #[must_use]
    pub fn last_seq(&self) -> EventSeq {
        self.seq
    }

    /// Kaynak olcumunun o andaki hali (7.2).
    #[must_use]
    pub fn resource(&self) -> &ResourceGauge {
        &self.resource
    }

    // -- hidrasyon ----------------------------------------------------------

    /// Acilista DB'den okunan goruntuyu cekirdege yukler (crash-only, 8.1).
    ///
    /// Kimlik sayaclari en buyuk kimligin uzerine tasinir ki yeniden baslatma
    /// sonrasi cakisma olmasin.
    pub fn hydrate(&mut self, snapshot: SystemSnapshot) {
        self.agents = snapshot.agents.into_iter().map(|a| (a.id, a)).collect();
        self.tasks = snapshot.tasks.into_iter().map(|t| (t.id, t)).collect();
        self.providers = snapshot.providers.into_iter().map(|p| (p.id, p)).collect();
        self.next_agent_id = self.agents.keys().copied().max().unwrap_or(0) + 1;
        self.next_task_id = self.tasks.keys().copied().max().unwrap_or(0) + 1;
        self.resource = snapshot.resource;
        self.recount();
    }

    /// Saglayici satirlarini tazeler (`omni-provider` yazar, cekirdek gosterir).
    pub fn set_providers(&mut self, providers: Vec<ProviderView>) {
        self.providers = providers.into_iter().map(|p| (p.id, p)).collect();
    }

    // -- yazma sozlesmesi ---------------------------------------------------

    /// UI komutunu uygular ve yayilacak olaylari dondurur (6.2).
    ///
    /// Hata durumunda **hicbir alan degismez**: her komut once dogrulanir,
    /// sonra mutasyon yapilir.
    ///
    /// # Errors
    /// Komut bilinmeyen bir varliga isaret ediyorsa, durum makinesi gecisi
    /// gecersizse, butce zarfi ya da derinlik tavani asiliyorsa hata doner.
    pub fn apply(&mut self, cmd: Command) -> Result<Vec<StateEvent>, CoreError> {
        match cmd {
            Command::SpawnTask {
                title,
                mode,
                parent_id,
                persona,
                duration_target,
                budget,
            } => self.spawn_task(title, mode, parent_id, persona, duration_target, budget),
            Command::WriteToAgent { agent_id, content } => self.write_to_agent(agent_id, &content),
            Command::Interrupt {
                agent_id,
                kind,
                source,
                reason,
            } => self.interrupt(agent_id, kind, source, reason),
            Command::Approve {
                agent_id,
                capability,
                target,
                decision,
                approver,
            } => self.approve(agent_id, &capability, &target, decision, &approver),
            Command::SetRouting {
                policy,
                strategy,
                config,
            } => self.set_routing(policy, strategy, config),
        }
    }

    /// Alt katmanlardan gelen olayi cekirdege isler.
    ///
    /// Gecersiz gecis ya da bilinmeyen varlik **sessizce kabul edilmez**:
    /// durum degismez, olay dusurulur ve gerekce kayda gecer. Hatayi cagiran
    /// tarafta gormek icin [`CoreState::try_ingest`] kullanilir.
    pub fn ingest(&mut self, ev: StateEvent) {
        let kind = ev.kind();
        if let Err(err) = self.try_ingest(ev) {
            tracing::warn!(event = kind, %err, "cekirdek olayi reddetti");
        }
    }

    /// [`CoreState::ingest`]'in hatayi tasiyan hali.
    ///
    /// # Errors
    /// Olay bilinmeyen bir ajana/goreve aitse ya da tasidigi durum gecisi
    /// makineye uymuyorsa hata doner; durum degismez.
    pub fn try_ingest(&mut self, ev: StateEvent) -> Result<(), CoreError> {
        match ev {
            StateEvent::AgentUpserted(view) => self.ingest_agent(view),
            StateEvent::TaskUpserted(view) => {
                self.next_task_id = self.next_task_id.max(view.id + 1);
                self.tasks.insert(view.id, view);
                Ok(())
            }
            StateEvent::FileTouched(touch) => self.require_agent(touch.agent_id).map(|_| ()),
            StateEvent::ToolCall(call) => self.require_agent(call.agent_id).map(|_| ()),
            StateEvent::Interrupt(view) => self.ingest_interrupt(view),
            // Bildirimlerin durum karsiligi yoktur; yalnizca yayilir.
            StateEvent::Notice(_) => Ok(()),
            StateEvent::ResourceTick(gauge) => {
                self.ingest_gauge(gauge);
                Ok(())
            }
        }
    }

    /// Ajani yeni duruma tasir.
    ///
    /// Ayni duruma gecis no-op'tur (`Ok(None)`), olay uretmez. Sonlanmis
    /// duruma gecerken ajan `Existing` tier'a duser: baglam artik RAM'de
    /// tutulmaz (7.1).
    ///
    /// # Errors
    /// Ajan yoksa ya da gecis makineye uymuyorsa hata doner.
    pub fn transition(
        &mut self,
        agent_id: AgentId,
        next: AgentState,
    ) -> Result<Option<StateEvent>, CoreError> {
        let current = self.require_agent(agent_id)?.state;
        if current == next {
            return Ok(None);
        }
        if !AgentStateMachine::allows(current, next) {
            return Err(CoreError::InvalidTransition {
                agent_id,
                prev: current,
                next,
            });
        }
        let seq = self.bump_seq();
        let Some(agent) = self.agents.get_mut(&agent_id) else {
            return Err(CoreError::UnknownAgent(agent_id));
        };
        agent.state = next;
        agent.last_event_seq = seq;
        if next.is_terminal() {
            agent.tier = AgentTier::Existing;
        }
        let event = StateEvent::AgentUpserted(agent.clone());
        self.recount();
        Ok(Some(event))
    }

    /// Ajanin tier'ini degistirir (scheduler admission karari, 7.1).
    ///
    /// # Errors
    /// Ajan yoksa ya da sonlanmis bir ajan yeniden yerlesik yapilmak
    /// isteniyorsa hata doner.
    pub fn set_tier(
        &mut self,
        agent_id: AgentId,
        tier: AgentTier,
    ) -> Result<Option<StateEvent>, CoreError> {
        let agent = self.require_agent(agent_id)?;
        if agent.tier == tier {
            return Ok(None);
        }
        if agent.state.is_terminal() && tier.is_resident() {
            return Err(CoreError::AgentTerminal {
                agent_id,
                state: agent.state,
            });
        }
        let seq = self.bump_seq();
        let Some(agent) = self.agents.get_mut(&agent_id) else {
            return Err(CoreError::UnknownAgent(agent_id));
        };
        agent.tier = tier;
        agent.last_event_seq = seq;
        let event = StateEvent::AgentUpserted(agent.clone());
        self.recount();
        Ok(Some(event))
    }

    /// Gorevi kapatir; durum etiketi cagirandan gelir (serbest `TEXT`).
    ///
    /// # Errors
    /// Gorev yoksa hata doner.
    pub fn close_task(
        &mut self,
        task_id: TaskId,
        status: impl Into<String>,
    ) -> Result<Vec<StateEvent>, CoreError> {
        let status = status.into();
        if status.trim().is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "close_task",
                reason: "gorev durumu bos".to_owned(),
            });
        }
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return Err(CoreError::UnknownTask(task_id));
        };
        task.status = status;
        task.closed_at = Some(now());
        Ok(vec![StateEvent::TaskUpserted(task.clone())])
    }

    /// Harcamayi goreve ve tum ust gorevlerine isler (AS4 butce zarfi).
    ///
    /// # Errors
    /// Gorev yoksa ya da tutar sonlu/pozitif degilse hata doner.
    pub fn charge(&mut self, task_id: TaskId, amount: f64) -> Result<Vec<StateEvent>, CoreError> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(CoreError::InvalidCommand {
                command: "charge",
                reason: "harcama tutari sonlu ve negatif olmayan olmali".to_owned(),
            });
        }
        if !self.tasks.contains_key(&task_id) {
            return Err(CoreError::UnknownTask(task_id));
        }
        let mut events = Vec::new();
        let mut cursor = Some(task_id);
        while let Some(id) = cursor {
            let Some(task) = self.tasks.get_mut(&id) else {
                break;
            };
            task.budget_spent = Some(task.budget_spent.unwrap_or(0.0) + amount);
            events.push(StateEvent::TaskUpserted(task.clone()));
            cursor = task.parent_id;
        }
        Ok(events)
    }

    // -- komut isleyicileri -------------------------------------------------

    fn spawn_task(
        &mut self,
        title: String,
        mode: String,
        parent_id: Option<TaskId>,
        persona: Option<String>,
        duration_target: Option<String>,
        budget: Option<f64>,
    ) -> Result<Vec<StateEvent>, CoreError> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "spawn_task",
                reason: "gorev basligi bos".to_owned(),
            });
        }
        let mode = mode.trim().to_owned();
        if mode.is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "spawn_task",
                reason: "gorev modu bos".to_owned(),
            });
        }
        if let Some(b) = budget
            && (!b.is_finite() || b < 0.0)
        {
            return Err(CoreError::InvalidCommand {
                command: "spawn_task",
                reason: "butce sonlu ve negatif olmayan olmali".to_owned(),
            });
        }

        // Ebeveyn cozulur: derinlik ve devralinan zarf buradan gelir.
        let (parent_root, depth, parent_remaining) = match parent_id {
            Some(pid) => {
                let parent = self.tasks.get(&pid).ok_or(CoreError::UnknownTask(pid))?;
                let depth =
                    parent
                        .depth
                        .checked_add(1)
                        .ok_or_else(|| CoreError::DepthExceeded {
                            depth: u16::from(parent.depth) + 1,
                            cap: u8::MAX,
                        })?;
                (Some(parent.root_id), depth, parent.budget_remaining())
            }
            None => (None, 0, None),
        };
        if let Some(cap) = self.depth_cap
            && depth > cap
        {
            return Err(CoreError::DepthExceeded {
                depth: u16::from(depth),
                cap,
            });
        }

        // Zarf: acikca verilen tutar ebeveynin kalanini asamaz; verilmezse
        // ebeveynden devralinir, kok gorevde `None` (sinirsiz) kalir (AS4/K11).
        let allocated = match (budget, parent_remaining) {
            (Some(istenen), Some(kalan)) if istenen > kalan => {
                return Err(CoreError::BudgetExceeded {
                    task_id: parent_id.unwrap_or_default(),
                    requested: istenen,
                    remaining: kalan,
                });
            }
            (Some(istenen), _) => Some(istenen),
            (None, devralinan) => devralinan,
        };

        let persona = persona
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| self.default_persona.clone());

        let task_id = self.alloc_task_id();
        let task = TaskView {
            id: task_id,
            parent_id,
            root_id: parent_root.unwrap_or(task_id),
            title,
            mode,
            status: TASK_STATUS_OPEN.to_owned(),
            depth,
            budget_allocated: allocated,
            budget_spent: Some(0.0),
            duration_target,
            created_at: now(),
            closed_at: None,
        };
        self.tasks.insert(task_id, task.clone());

        let agent_id = self.alloc_agent_id();
        let seq = self.bump_seq();
        let agent = AgentView {
            id: agent_id,
            persona,
            // Yeni ajan slot bekler; `Active`'e gecirme karari valinindir (7.2).
            tier: AgentTier::Queued,
            task_id,
            parent_id: None,
            state: AgentState::Init,
            rss_kb: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            trust: INITIAL_TRUST,
            depth,
            last_event_seq: seq,
        };
        self.agents.insert(agent_id, agent.clone());

        Ok(vec![
            StateEvent::TaskUpserted(task),
            StateEvent::AgentUpserted(agent),
            self.resource_event(),
        ])
    }

    fn write_to_agent(
        &mut self,
        agent_id: AgentId,
        content: &str,
    ) -> Result<Vec<StateEvent>, CoreError> {
        if content.trim().is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "write_to_agent",
                reason: "mesaj govdesi bos".to_owned(),
            });
        }
        let agent = self.require_agent(agent_id)?;
        if agent.state.is_terminal() {
            return Err(CoreError::AgentTerminal {
                agent_id,
                state: agent.state,
            });
        }
        let task_id = agent.task_id;
        let resume = matches!(agent.state, AgentState::Blocked | AgentState::Interrupted);

        let mut events = vec![StateEvent::Notice(
            NoticeView::new(
                NoticeLevel::Info,
                "agent_message",
                "kullanici mesaji ajana iletildi",
                now(),
            )
            .with_agent(agent_id)
            .with_task(task_id),
        )];
        // Bekleyen ajan insan girdisiyle cozulur ve plana doner (6.8).
        if resume && let Some(ev) = self.transition(agent_id, AgentState::Planning)? {
            events.push(ev);
        }
        Ok(events)
    }

    fn interrupt(
        &mut self,
        agent_id: AgentId,
        kind: String,
        source: String,
        reason: Option<String>,
    ) -> Result<Vec<StateEvent>, CoreError> {
        let kind = kind.trim().to_owned();
        let source = source.trim().to_owned();
        if kind.is_empty() || source.is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "interrupt",
                reason: "mudahale turu ve kaynagi zorunlu".to_owned(),
            });
        }
        let agent = self.require_agent(agent_id)?;
        if agent.state.is_terminal() {
            return Err(CoreError::AgentTerminal {
                agent_id,
                state: agent.state,
            });
        }
        let task_id = agent.task_id;

        let view = InterruptView {
            id: None,
            agent_id,
            kind,
            source,
            reason,
            ts: now(),
            resolved_at: None,
        };
        self.interrupts.push(view.clone());

        let mut events = vec![
            StateEvent::Interrupt(view),
            StateEvent::Notice(
                NoticeView::new(
                    NoticeLevel::Warn,
                    "agent_interrupted",
                    "ajan disaridan kesildi",
                    now(),
                )
                .with_agent(agent_id)
                .with_task(task_id),
            ),
        ];
        if let Some(ev) = self.transition(agent_id, AgentState::Interrupted)? {
            events.push(ev);
        }
        Ok(events)
    }

    fn approve(
        &mut self,
        agent_id: AgentId,
        capability: &str,
        target: &str,
        decision: omni_proto::ApprovalDecision,
        approver: &str,
    ) -> Result<Vec<StateEvent>, CoreError> {
        if capability.trim().is_empty() || target.trim().is_empty() || approver.trim().is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "approve",
                reason: "yetki, hedef ve onaylayan zorunlu".to_owned(),
            });
        }
        let agent = self.require_agent(agent_id)?;
        if agent.state.is_terminal() {
            return Err(CoreError::AgentTerminal {
                agent_id,
                state: agent.state,
            });
        }
        let task_id = agent.task_id;
        let blocked = agent.state == AgentState::Blocked;

        let allow = decision == omni_proto::ApprovalDecision::Allow;
        let notice = if allow {
            NoticeView::new(
                NoticeLevel::Info,
                "capability_allowed",
                format!("'{capability}' -> '{target}' onaylandi ({approver})"),
                now(),
            )
        } else {
            NoticeView::new(
                NoticeLevel::Warn,
                "capability_denied",
                format!("'{capability}' -> '{target}' reddedildi ({approver})"),
                now(),
            )
        };
        let mut events = vec![StateEvent::Notice(
            notice.with_agent(agent_id).with_task(task_id),
        )];

        // Onay bekleyen ajan cozulur: izin verildiyse cagri surer, reddedildiyse
        // ajan plana donup baska yol arar (K3).
        if blocked {
            let next = if allow {
                AgentState::RunningTool
            } else {
                AgentState::Planning
            };
            if let Some(ev) = self.transition(agent_id, next)? {
                events.push(ev);
            }
        }
        Ok(events)
    }

    fn set_routing(
        &mut self,
        policy: String,
        strategy: RoutingStrategy,
        config: serde_json::Value,
    ) -> Result<Vec<StateEvent>, CoreError> {
        let policy = policy.trim().to_owned();
        if policy.is_empty() {
            return Err(CoreError::InvalidCommand {
                command: "set_routing",
                reason: "politika adi bos".to_owned(),
            });
        }
        if !config.is_object() {
            return Err(CoreError::InvalidCommand {
                command: "set_routing",
                reason: "yonlendirme yapilandirmasi JSON nesnesi olmali".to_owned(),
            });
        }
        let message = format!("yonlendirme politikasi '{policy}' guncellendi");
        self.routing = Some(RoutingSetting {
            policy,
            strategy,
            config,
            updated_at: now(),
        });
        Ok(vec![StateEvent::Notice(NoticeView::new(
            NoticeLevel::Info,
            "routing_updated",
            message,
            now(),
        ))])
    }

    // -- olay isleyicileri --------------------------------------------------

    fn ingest_agent(&mut self, view: AgentView) -> Result<(), CoreError> {
        if let Some(current) = self.agents.get(&view.id)
            && current.state != view.state
            && !AgentStateMachine::allows(current.state, view.state)
        {
            return Err(CoreError::InvalidTransition {
                agent_id: view.id,
                prev: current.state,
                next: view.state,
            });
        }
        self.next_agent_id = self.next_agent_id.max(view.id + 1);
        self.seq = self.seq.max(view.last_event_seq);
        self.agents.insert(view.id, view);
        self.recount();
        Ok(())
    }

    fn ingest_interrupt(&mut self, view: InterruptView) -> Result<(), CoreError> {
        self.require_agent(view.agent_id)?;
        if let Some(resolved) = view.resolved_at {
            // Cozulme bildirimi: ayni ajanin acik kaydini kapatir.
            if let Some(open) = self
                .interrupts
                .iter_mut()
                .rev()
                .find(|i| i.agent_id == view.agent_id && i.is_open())
            {
                open.resolved_at = Some(resolved);
                return Ok(());
            }
        }
        self.interrupts.push(view);
        Ok(())
    }

    fn ingest_gauge(&mut self, gauge: ResourceGauge) {
        // Tier sayimlarinin sahibi cekirdektir; olcumden yalnizca donanim
        // okumalari ve admission karari alinir (7.2).
        self.resource.rss_kb = gauge.rss_kb;
        self.resource.rss_limit_kb = gauge.rss_limit_kb;
        self.resource.cpu_pct = gauge.cpu_pct;
        self.resource.open_fds = gauge.open_fds;
        self.resource.fd_limit = gauge.fd_limit;
        self.resource.admission_open = gauge.admission_open;
        self.resource.ts = gauge.ts;
        self.recount();
    }

    // -- ic yardimcilar -----------------------------------------------------

    fn require_agent(&self, agent_id: AgentId) -> Result<&AgentView, CoreError> {
        self.agents
            .get(&agent_id)
            .ok_or(CoreError::UnknownAgent(agent_id))
    }

    fn alloc_agent_id(&mut self) -> AgentId {
        let id = self.next_agent_id;
        self.next_agent_id = id.saturating_add(1);
        id
    }

    fn alloc_task_id(&mut self) -> TaskId {
        let id = self.next_task_id;
        self.next_task_id = id.saturating_add(1);
        id
    }

    fn bump_seq(&mut self) -> EventSeq {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn recount(&mut self) {
        let (mut active, mut queued, mut sleeping, mut existing) = (0_u32, 0_u32, 0_u32, 0_u32);
        for agent in self.agents.values() {
            let slot = match agent.tier {
                AgentTier::Active => &mut active,
                AgentTier::Queued => &mut queued,
                AgentTier::Sleeping => &mut sleeping,
                AgentTier::Existing => &mut existing,
            };
            *slot = slot.saturating_add(1);
        }
        self.resource.active = active;
        self.resource.queued = queued;
        self.resource.sleeping = sleeping;
        self.resource.existing = existing;
    }

    fn resource_event(&mut self) -> StateEvent {
        self.recount();
        self.resource.ts = now();
        StateEvent::ResourceTick(self.resource.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::ApprovalDecision;

    fn spawn(core: &mut CoreState, title: &str, parent: Option<TaskId>) -> Vec<StateEvent> {
        core.apply(Command::SpawnTask {
            title: title.into(),
            mode: "user_driven".into(),
            parent_id: parent,
            persona: None,
            duration_target: None,
            budget: None,
        })
        .expect("gorev acilmali")
    }

    #[test]
    fn gorev_acilinca_ajan_ve_olcum_uretilir() {
        let mut core = CoreState::new();
        let events = spawn(&mut core, "dikey dilim", None);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind(), "task_upserted");
        assert_eq!(events[1].kind(), "agent_upserted");
        assert_eq!(events[2].kind(), "resource_tick");

        let snap = core.snapshot();
        assert_eq!(snap.tasks.len(), 1);
        assert_eq!(snap.agents.len(), 1);
        assert_eq!(snap.resource.queued, 1);
        assert_eq!(snap.resource.total_agents(), 1);
        let agent = snap.agents.first().expect("ajan");
        assert_eq!(agent.state, AgentState::Init);
        assert_eq!(agent.persona, DEFAULT_PERSONA);
        assert!(agent.tier.is_resident());
    }

    #[test]
    fn bos_baslik_reddedilir() {
        let mut core = CoreState::new();
        let sonuc = core.apply(Command::SpawnTask {
            title: "   ".into(),
            mode: "user_driven".into(),
            parent_id: None,
            persona: None,
            duration_target: None,
            budget: None,
        });
        assert!(matches!(sonuc, Err(CoreError::InvalidCommand { .. })));
        assert_eq!(core.snapshot().tasks.len(), 0);
    }

    #[test]
    fn bilinmeyen_ebeveyn_reddedilir() {
        let mut core = CoreState::new();
        let sonuc = core.apply(Command::SpawnTask {
            title: "alt".into(),
            mode: "user_driven".into(),
            parent_id: Some(99),
            persona: None,
            duration_target: None,
            budget: None,
        });
        assert!(matches!(sonuc, Err(CoreError::UnknownTask(99))));
    }

    #[test]
    fn alt_gorev_derinlik_ve_zarf_devralir() {
        let mut core = CoreState::new();
        core.apply(Command::SpawnTask {
            title: "kok".into(),
            mode: "autonomous".into(),
            parent_id: None,
            persona: Some("planner".into()),
            duration_target: Some("mvp".into()),
            budget: Some(10.0),
        })
        .expect("kok gorev");

        spawn(&mut core, "alt", Some(1));
        let alt = core.task(2).expect("alt gorev");
        assert_eq!(alt.depth, 1);
        assert_eq!(alt.root_id, 1);
        assert_eq!(alt.budget_allocated, Some(10.0));
    }

    #[test]
    fn zarf_asimi_reddedilir() {
        let mut core = CoreState::new();
        core.apply(Command::SpawnTask {
            title: "kok".into(),
            mode: "autonomous".into(),
            parent_id: None,
            persona: None,
            duration_target: None,
            budget: Some(1.0),
        })
        .expect("kok gorev");

        let sonuc = core.apply(Command::SpawnTask {
            title: "alt".into(),
            mode: "autonomous".into(),
            parent_id: Some(1),
            persona: None,
            duration_target: None,
            budget: Some(5.0),
        });
        assert!(matches!(sonuc, Err(CoreError::BudgetExceeded { .. })));
        assert!(core.task(2).is_none());
    }

    #[test]
    fn derinlik_tavani_zorlanir() {
        let mut core = CoreState::new().with_depth_cap(1);
        spawn(&mut core, "kok", None);
        spawn(&mut core, "alt", Some(1));
        let sonuc = core.apply(Command::SpawnTask {
            title: "torun".into(),
            mode: "user_driven".into(),
            parent_id: Some(2),
            persona: None,
            duration_target: None,
            budget: None,
        });
        assert!(matches!(
            sonuc,
            Err(CoreError::DepthExceeded { depth: 2, cap: 1 })
        ));
    }

    #[test]
    fn gecersiz_gecis_reddedilir() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Planning).expect("plan");
        core.transition(1, AgentState::Done).expect("bitir");

        let sonuc = core.transition(1, AgentState::Planning);
        assert!(matches!(sonuc, Err(CoreError::InvalidTransition { .. })));
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::Done);
    }

    #[test]
    fn init_dogrudan_tool_calistiramaz() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        assert!(matches!(
            core.transition(1, AgentState::RunningTool),
            Err(CoreError::InvalidTransition { .. })
        ));
        assert!(!AgentStateMachine::allows(
            AgentState::Interrupted,
            AgentState::RunningTool
        ));
        assert!(AgentStateMachine::successors(AgentState::Failed).is_empty());
    }

    #[test]
    fn ayni_duruma_gecis_olay_uretmez() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Planning).expect("plan");
        assert!(
            core.transition(1, AgentState::Planning)
                .expect("no-op")
                .is_none()
        );
    }

    #[test]
    fn sonlanan_ajan_tier_dusurur() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Planning).expect("plan");
        core.transition(1, AgentState::Failed).expect("hata");
        let agent = core.agent(1).expect("ajan");
        assert_eq!(agent.tier, AgentTier::Existing);
        assert!(!agent.is_resident());
        assert_eq!(core.resource().queued, 0);
        assert_eq!(core.resource().existing, 1);
    }

    #[test]
    fn sonlanmis_ajan_yeniden_yerlesik_yapilamaz() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Planning).expect("plan");
        core.transition(1, AgentState::Done).expect("bitir");
        assert!(matches!(
            core.set_tier(1, AgentTier::Active),
            Err(CoreError::AgentTerminal { .. })
        ));
    }

    #[test]
    fn mudahale_kaydi_acilir_ve_ajan_kesilir() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Planning).expect("plan");

        let events = core
            .apply(Command::Interrupt {
                agent_id: 1,
                kind: "pause".into(),
                source: "tui".into(),
                reason: Some("elle".into()),
            })
            .expect("mudahale");
        assert_eq!(events[0].kind(), "interrupt");
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::Interrupted);
        assert_eq!(core.open_interrupts().count(), 1);
    }

    #[test]
    fn sonlanmis_ajana_mudahale_reddedilir() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Planning).expect("plan");
        core.transition(1, AgentState::Done).expect("bitir");
        assert!(matches!(
            core.apply(Command::Interrupt {
                agent_id: 1,
                kind: "pause".into(),
                source: "tui".into(),
                reason: None,
            }),
            Err(CoreError::AgentTerminal { .. })
        ));
    }

    #[test]
    fn kullanici_mesaji_bekleyen_ajani_cozer() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Blocked).expect("blok");

        let events = core
            .apply(Command::WriteToAgent {
                agent_id: 1,
                content: "devam et".into(),
            })
            .expect("mesaj");
        assert_eq!(events.len(), 2);
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::Planning);
    }

    #[test]
    fn bilinmeyen_ajana_yazilamaz() {
        let mut core = CoreState::new();
        assert!(matches!(
            core.apply(Command::WriteToAgent {
                agent_id: 7,
                content: "selam".into(),
            }),
            Err(CoreError::UnknownAgent(7))
        ));
    }

    #[test]
    fn onay_bekleyen_ajani_surdurur() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Blocked).expect("blok");
        core.apply(Command::Approve {
            agent_id: 1,
            capability: "shell".into(),
            target: "ls".into(),
            decision: ApprovalDecision::Allow,
            approver: "insan".into(),
        })
        .expect("onay");
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::RunningTool);
    }

    #[test]
    fn ret_karari_ajani_plana_dondurur() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        core.transition(1, AgentState::Blocked).expect("blok");
        core.apply(Command::Approve {
            agent_id: 1,
            capability: "shell".into(),
            target: "rm".into(),
            decision: ApprovalDecision::Deny,
            approver: "insan".into(),
        })
        .expect("ret");
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::Planning);
    }

    #[test]
    fn yonlendirme_ayari_saklanir() {
        let mut core = CoreState::new();
        let events = core
            .apply(Command::SetRouting {
                policy: "default".into(),
                strategy: RoutingStrategy::Jep,
                config: serde_json::json!({ "roles": {} }),
            })
            .expect("politika");
        assert_eq!(events[0].kind(), "notice");
        let ayar = core.routing().expect("ayar");
        assert_eq!(ayar.policy, "default");
        assert_eq!(ayar.strategy, RoutingStrategy::Jep);

        assert!(matches!(
            core.apply(Command::SetRouting {
                policy: "default".into(),
                strategy: RoutingStrategy::Jep,
                config: serde_json::json!([]),
            }),
            Err(CoreError::InvalidCommand { .. })
        ));
    }

    #[test]
    fn ingest_gecersiz_gecisi_dusurur() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        let mut view = core.agent(1).expect("ajan").clone();
        view.state = AgentState::RunningTool;

        let sonuc = core.try_ingest(StateEvent::AgentUpserted(view.clone()));
        assert!(matches!(sonuc, Err(CoreError::InvalidTransition { .. })));
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::Init);

        // Panik uretmeyen sarmalayici da durumu degistirmez.
        core.ingest(StateEvent::AgentUpserted(view));
        assert_eq!(core.agent(1).expect("ajan").state, AgentState::Init);
    }

    #[test]
    fn ingest_gecerli_gecisi_uygular() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        let mut view = core.agent(1).expect("ajan").clone();
        view.state = AgentState::Planning;
        view.tokens_in = 120;
        core.ingest(StateEvent::AgentUpserted(view));
        let agent = core.agent(1).expect("ajan");
        assert_eq!(agent.state, AgentState::Planning);
        assert_eq!(agent.tokens_in, 120);
    }

    #[test]
    fn ingest_bilinmeyen_ajanin_dosya_dokunusunu_reddeder() {
        let mut core = CoreState::new();
        let touch = omni_proto::FileTouch {
            id: None,
            agent_id: 4,
            path: "a.rs".into(),
            outside_workspace: false,
            added: 1,
            removed: 0,
            pre_ref: None,
            post_ref: None,
            ts: now(),
        };
        assert!(matches!(
            core.try_ingest(StateEvent::FileTouched(touch)),
            Err(CoreError::UnknownAgent(4))
        ));
    }

    #[test]
    fn olcum_donanim_alanlarini_tazeler_sayimi_korur() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        let mut gauge = ResourceGauge::at(now());
        gauge.rss_kb = 4096;
        gauge.active = 99; // yanlis sayim: cekirdek kendi sayimini korur
        gauge.admission_open = false;
        core.ingest(StateEvent::ResourceTick(gauge));
        assert_eq!(core.resource().rss_kb, 4096);
        assert_eq!(core.resource().active, 0);
        assert_eq!(core.resource().queued, 1);
        assert!(!core.resource().admission_open);
    }

    #[test]
    fn harcama_ust_gorevlere_yansir() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        spawn(&mut core, "alt", Some(1));
        let events = core.charge(2, 1.5).expect("harcama");
        assert_eq!(events.len(), 2);
        assert_eq!(core.task(1).expect("kok").budget_spent, Some(1.5));
        assert_eq!(core.task(2).expect("alt").budget_spent, Some(1.5));
        assert!(core.charge(2, f64::NAN).is_err());
        assert!(matches!(
            core.charge(9, 1.0),
            Err(CoreError::UnknownTask(9))
        ));
    }

    #[test]
    fn gorev_kapanisi_damgalanir() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        let events = core.close_task(1, "done").expect("kapanis");
        assert_eq!(events.len(), 1);
        let task = core.task(1).expect("gorev");
        assert_eq!(task.status, "done");
        assert!(task.closed_at.is_some());
        assert!(core.close_task(1, "  ").is_err());
    }

    #[test]
    fn hidrasyon_kimlik_sayaclarini_tasir() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        let snap = core.snapshot();

        let mut yeni = CoreState::new();
        yeni.hydrate(snap);
        assert_eq!(yeni.snapshot().agents.len(), 1);
        spawn(&mut yeni, "ikinci", None);
        assert!(yeni.task(2).is_some());
        assert!(yeni.agent(2).is_some());
    }

    #[test]
    fn olay_sirasi_monoton_artar() {
        let mut core = CoreState::new();
        spawn(&mut core, "kok", None);
        let ilk = core.last_seq();
        core.transition(1, AgentState::Planning).expect("plan");
        assert!(core.last_seq() > ilk);
        assert_eq!(core.agent(1).expect("ajan").last_event_seq, core.last_seq());
    }
}
