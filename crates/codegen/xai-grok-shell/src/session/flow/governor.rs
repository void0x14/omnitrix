//! Flow Governor — akış kompozisyonu ve seam API'si.
//!
//! SessionActor bu yapıyı `Arc<parking_lot::Mutex<FlowGovernor>>` olarak tutar
//! (PlanModeTracker deseni) ve dört karar noktasında çağırır:
//! A) handle_prompt → activate, B) tool filtresi → tool_definitions_filter,
//! C) stop gate → stop_decision, D) goal round → round_decision.
//! Checkpoint aracıyla köprü: paylaşımlı `CheckpointCell` (Arc<Mutex>).

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use super::classifier::{ClassifiedFlow, FlowClassifier, FlowRules, UserMode};
use super::definition::{ArtifactId, FlowDefinition, StageId};
use super::duration::{DurationDecision, FlowDurationSystem};
use super::events::FlowEvents;
use super::gate::FlowGate;
use super::state::{FlowPhase, FlowStateMachine};
use super::store::FlowStore;

/// Checkpoint aracı ile session arasındaki paylaşımlı köprü.
#[derive(Debug, Default)]
pub struct CheckpointCell {
    pub active: bool,
    pub stage: Option<StageId>,
    pub request: Option<CheckpointRequest>,
    pub result: Option<CheckpointResult>,
    pub redirects: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointRequest {
    pub stage: String,
    pub summary: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointResult {
    pub accepted: bool,
    pub pending: bool,
    pub stage: String,
    pub directive: String,
    pub detail: String,
}

impl CheckpointResult {
    fn pending() -> Self {
        Self {
            accepted: false,
            pending: true,
            stage: String::new(),
            directive: "checkpoint işleniyor; tekrar çağır".to_string(),
            detail: String::new(),
        }
    }

    fn done(accepted: bool, stage: &StageId, directive: &str, detail: String) -> Self {
        Self {
            accepted,
            pending: false,
            stage: stage.as_str().to_string(),
            directive: directive.to_string(),
            detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundVerdict {
    Continue(String),
    EndTurn,
}

pub struct FlowGovernor {
    machine: FlowStateMachine,
    store: FlowStore,
    events: FlowEvents,
    cell: std::sync::Arc<Mutex<CheckpointCell>>,
    classified: Option<ClassifiedFlow>,
    user_mode: UserMode,
    rules: FlowRules,
}

impl FlowGovernor {
    pub fn new(session_dir: &Path) -> Self {
        let cell = std::sync::Arc::new(Mutex::new(CheckpointCell::default()));
        Self {
            machine: FlowStateMachine::idle(),
            store: FlowStore::open(session_dir),
            events: FlowEvents::new(session_dir),
            cell,
            classified: None,
            user_mode: UserMode::UserOriented,
            rules: FlowRules::default(),
        }
    }

    /// Paylaşımlı köprü (checkpoint aracına enjekte edilir).
    pub fn cell(&self) -> std::sync::Arc<Mutex<CheckpointCell>> {
        self.cell.clone()
    }

    pub fn is_active(&self) -> bool {
        self.classified.is_some() && !self.machine.is_complete()
    }

    pub fn flow(&self) -> Option<&'static FlowDefinition> {
        self.classified.map(|c| c.flow())
    }

    // ── A) handle_prompt aktivasyonu ──────────────────────────────────────
    /// Görevi sınıflandırır, akışı başlatır, ilk aşama direktifini döndürür.
    /// Direct akışa düşen görevlerde dahi stop gate denetimi çalışır.
    pub fn activate(&mut self, prompt_text: &str, user_mode: UserMode) -> Option<String> {
        if self.is_active() {
            return None; // mevcut akış korunur
        }
        let classified = FlowClassifier::classify(prompt_text, user_mode, &self.rules);
        self.classified = Some(classified);
        self.user_mode = user_mode;
        let flow = classified.flow();
        let first = self.machine.start(flow);
        self.store.record(
            ArtifactId::ProblemList,
            true,
            format!("akış seçimi (sistem): {}", flow.name),
        );
        if classified != ClassifiedFlow::Direct {
            self.store.record(
                ArtifactId::DurationDecision,
                true,
                format!(
                    "süre kararı (sistem): {:?}",
                    FlowDurationSystem::decide(
                        prompt_text.len(),
                        &classified,
                        user_mode,
                        &self.rules
                    )
                ),
            );
        }
        self.cell.lock().active = true;
        self.cell.lock().stage = Some(first);
        self.events.phase_changed(first);
        Some(self.machine.stage_directive(first).to_string())
    }

    // ── B) tool görünürlük filtresi ───────────────────────────────────────
    pub fn tool_definitions_filter<T: Clone>(&self, defs: Vec<T>, id_of: impl Fn(&T) -> &str) -> Vec<T> {
        if !self.is_active() {
            return defs;
        }
        defs.into_iter()
            .filter(|d| FlowGate::tool_allowed(id_of(d), &self.machine))
            .collect()
    }

    // ── C) stop gate danışmanı ────────────────────────────────────────────
    /// Some(direktif) = KeepWorking (aşama kanıtı eksik), None = dur.
    pub fn stop_decision(&mut self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        if self.machine.bump_redirect() {
            let stage = self.machine.current_stage().unwrap_or(StageId::Do);
            let directive = self.machine.stage_directive(stage);
            let detail = format!("aşama {} tamamlanmadı; stop reddedildi", stage.as_str());
            self.store.record(ArtifactId::DurationDecision, false, detail.clone());
            self.events.violation("stop", &detail);
            Some(directive.to_string())
        } else {
            None // redirect limiti doldu → deterministik kilit açma
        }
    }

    // ── D) goal round danışmanı ───────────────────────────────────────────
    pub fn round_decision(&mut self) -> RoundVerdict {
        if !self.is_active() {
            return RoundVerdict::EndTurn;
        }
        match self.machine.current_stage() {
            None => RoundVerdict::EndTurn,
            Some(stage) => RoundVerdict::Continue(self.machine.stage_directive(stage).to_string()),
        }
    }

    // ── Tool başarı kancası (S6: handle_bridge_tool_success) ──────────────
    pub fn on_tool_success(&mut self, tool: &str) {
        if !self.is_active() {
            return;
        }
        // checkpoint isteği varsa doğrula. Arc klonu: guard `self`'i ödünç
        // almasın — `validate_checkpoint` `&mut self` ister (E0502 önlenir).
        let cell_arc = self.cell.clone();
        let mut cell = cell_arc.lock();
        if let Some(req) = cell.request.take() {
            let stage = cell.stage.unwrap_or(StageId::Do);
            let result = self.validate_checkpoint(&req, stage, &mut cell);
            cell.result = Some(result);
        }
        // ihlal kaydı: bu tool bu aşamada yasaklanmış bir gruba mı ait?
        if let Some(group) = super::gate::tool_group_of(tool) {
            if !self.machine.tool_unlocked(group) {
                drop(cell);
                self.events.violation(tool, "kilitli grup çağrısı (filtre aşıldı)");
            }
        }
    }

    /// Checkpoint doğrulaması — saf, hızlı; tool isteği + mevcut aşama + cwd.
    pub fn validate_checkpoint(
        &mut self,
        req: &CheckpointRequest,
        current: StageId,
        cell: &mut CheckpointCell,
    ) -> CheckpointResult {
        // aşama sırası kontrolü
        if req.stage != current.as_str() {
            self.events.checkpoint_rejected(&req.stage, "yanlış aşama");
            return CheckpointResult::done(
                false,
                &current,
                self.machine.stage_directive(current),
                format!("Bu aşama değil: sen {} aşamasındasın. {} aşamasına geçemezsin.", current.as_str(), req.stage),
            );
        }
        // kanıt dosyası kontrolü (isteğe bağlı files alanı)
        for f in &req.files {
            let p = Path::new(f);
            if !p.is_file() || p.metadata().map(|m| m.len() == 0).unwrap_or(true) {
                self.events.checkpoint_rejected(&req.stage, "kanıt dosyası eksik/boş");
                return CheckpointResult::done(
                    false,
                    &current,
                    self.machine.stage_directive(current),
                    format!("Kanıt dosyası eksik veya boş: {f}. Önce aşama çıktısını üret, sonra kapat.", ),
                );
            }
        }
        // kabul: kanıtları kaydet, aşamayı ilerlet
        if let Some(def) = self.machine.stage_def(current) {
            for artifact in def.produces {
                self.store.record(*artifact, true, req.summary.clone());
                self.machine.record_artifact(*artifact, true);
            }
        }
        let next = self.machine.current_stage();
        cell.stage = next;
        match next {
            None => {
                cell.active = false;
                self.events.completed();
                CheckpointResult::done(true, &current, "", "Görev tamamlandı.".to_string())
            }
            Some(ns) => {
                self.events.phase_changed(ns);
                CheckpointResult::done(
                    true,
                    &current,
                    self.machine.stage_directive(ns),
                    format!("Aşama tamam: {} → {}", current.as_str(), ns.as_str()),
                )
            }
        }
    }

    /// flow_checkpoint aracı çağrısını işler (S6): isteği hemen doğrular ve
    /// kararı (kabul / red + direktif) JSON metni olarak döndürür. Dönen
    /// metin tool sonucu olarak chat state'e gider; araç durumsuz olduğu
    /// için kararın ikinci bir çağrıya saklanması gerekmez. Akış yoksa
    /// açık bir mesaj döner (patlama yok — I6).
    pub fn process_checkpoint_call(&mut self, stage: &str, summary: &str, files: Vec<String>) -> String {
        if !self.is_active() {
            return "flow_checkpoint: aktif akış yok; bu araç yalnızca akış denetimli görevlerde kullanılabilir"
                .to_string();
        }
        let req = CheckpointRequest {
            stage: stage.to_string(),
            summary: summary.to_string(),
            files,
        };
        let cell_arc = self.cell.clone();
        let mut cell = cell_arc.lock();
        let current = cell.stage.unwrap_or(StageId::Do);
        let result = self.validate_checkpoint(&req, current, &mut cell);
        serde_json::to_string(&result).unwrap_or_else(|_| result.directive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("flow_gov_test_{}", std::process::id()));
        std::fs::create_dir_all(&d).ok();
        d
    }

    #[test]
    fn activate_returns_directive_and_classifies() {
        let mut g = FlowGovernor::new(&temp_dir());
        let d = g.activate("2026'da en iyi rust frameworkü nedir araştır", UserMode::UserOriented);
        assert!(d.is_some());
        assert_eq!(g.flow().map(|f| f.name), Some("universal"));
        assert!(g.is_active());
    }

    #[test]
    fn checkpoint_wrong_stage_rejected() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("2026'da en iyi rust frameworkü nedir araştır", UserMode::UserOriented);
        let cell = g.cell();
        let req = CheckpointRequest { stage: "research".to_string(), summary: "x".to_string(), files: vec![] };
        let res = g.validate_checkpoint(&req, StageId::Analyze, &mut *cell.lock());
        assert!(!res.accepted);
        assert!(!res.directive.is_empty());
    }

    #[test]
    fn stop_decision_blocks_until_done() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("selam", UserMode::UserOriented); // direct akış
        // direct akış: stop denetimi tamamlanana kadar engeller
        assert!(g.stop_decision().is_some());
    }
}
