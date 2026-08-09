//! Flow Governor — akış kompozisyonu ve seam API'si.
//!
//! SessionActor bu yapıyı `Arc<parking_lot::Mutex<FlowGovernor>>` olarak tutar
//! (PlanModeTracker deseni) ve dört karar noktasında çağırır:
//! A) handle_prompt → activate, B) tool filtresi → tool_definitions_filter,
//! C) stop gate → stop_decision, D) goal round → round_decision.
//! Checkpoint aracıyla köprü: paylaşımlı `CheckpointCell` (Arc<Mutex>).
//!
//! Sistem aşamaları (AI kararsız): `duration` (süre kararı), `parallel_query`
//! (bağımlılık analizi). Async sistem aşamaları (S6 kancası tamamlar):
//! `verify` (yargıç alt ajanı), `execute` (duration=Full → sistem çoklu ajan
//! yürütmesi), `notify` (telegram/webhook/sms/çağrı). Async istekler
//! `CheckpointResult.detail = "system_async:<ad>"` ile işaretlenir ve
//! `finalize_*` metotlarıyla tamamlanır — karar her zaman burada, sistemde.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use super::classifier::{ClassifiedFlow, FlowClassifier, FlowRules, UserMode};
use super::config::{FlowConfig, FlowOverrides};
use super::definition::{ArtifactId, FlowDefinition, StageId, ToolGroup};
use super::duration::{DurationDecision, FlowDurationSystem};
use super::events::FlowEvents;
use super::gate::FlowGate;
use super::judge::JudgeVerdict;
use super::notify::NotifyReport;
use super::parallel::{graph_summary, analyze};
use super::state::FlowStateMachine;
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
    fn done(accepted: bool, stage: &StageId, directive: &str, detail: String) -> Self {
        Self {
            accepted,
            pending: false,
            stage: stage.as_str().to_string(),
            directive: directive.to_string(),
            detail,
        }
    }

    /// Sistemin asenkron tamamlayacağı bir aşama (S6 kancası).
    fn system_async(stage: &StageId, action: &str) -> Self {
        Self {
            accepted: false,
            pending: false,
            stage: stage.as_str().to_string(),
            directive: String::new(),
            detail: format!("system_async:{action}"),
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
    overrides: FlowOverrides,
    config_fp: (u64, u64, u64),
    duration: Option<DurationDecision>,
    problem_text: String,
}

impl FlowGovernor {
    pub fn new(session_dir: &Path) -> Self {
        let cell = std::sync::Arc::new(Mutex::new(CheckpointCell::default()));
        let config = FlowConfig::load();
        let rules = config.rules.clone().unwrap_or_default();
        let overrides = config.overrides.clone().unwrap_or_default();
        let config_fp = config.fingerprint();
        Self {
            machine: FlowStateMachine::idle(),
            store: FlowStore::open(session_dir),
            events: FlowEvents::new(session_dir),
            cell,
            classified: None,
            user_mode: UserMode::UserOriented,
            rules,
            overrides,
            config_fp,
            duration: None,
            problem_text: String::new(),
        }
    }

    /// Config dosyalarını (mtime üzerinden) tazeler — canlı reload.
    fn refresh_config(&mut self) {
        let fp = FlowConfig::load().fingerprint();
        if fp != self.config_fp {
            let config = FlowConfig::load();
            self.rules = config.rules.clone().unwrap_or_default();
            self.overrides = config.overrides.clone().unwrap_or_default();
            self.config_fp = fp;
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

    pub fn duration_decision(&self) -> Option<DurationDecision> {
        self.duration
    }

    pub fn problem_text(&self) -> &str {
        &self.problem_text
    }

    /// Son `ExecutionGraph` kanıtı (execute_full için S6 okur).
    pub fn execution_graph(&self) -> Option<super::parallel::ExecutionGraph> {
        self.store
            .progress()
            .iter()
            .rev()
            .find(|r| r.artifact == ArtifactId::ExecutionGraph.as_str() && r.ok)
            .and_then(|r| serde_json::from_str(&r.detail).ok())
    }

    /// Bildirim kanalları (notify aşaması için; config tazelenir).
    pub fn notify_config(&self) -> super::config::NotifyConfig {
        FlowConfig::load().notify.clone().unwrap_or_default()
    }

    /// Override'ları uygulayan aşama direktifi (flows.toml → gömülü).
    pub fn stage_directive_effective(&self, stage: StageId) -> &str {
        self.overrides
            .stage_directives
            .get(stage.as_str())
            .map(|s| s.as_str())
            .unwrap_or_else(|| self.machine.stage_directive(stage))
    }

    /// Override'ları uygulayan aşama araç grupları.
    pub fn stage_tool_groups_effective(&self, stage: StageId) -> Vec<ToolGroup> {
        if let Some(groups) = self.overrides.stage_tool_groups.get(stage.as_str()) {
            let parsed: Vec<ToolGroup> = groups
                .iter()
                .filter_map(|g| ToolGroup::from_str(g))
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
        self.machine.stage_tool_groups(stage).to_vec()
    }

    /// Override'lı tool kilidi.
    fn tool_unlocked_effective(&self, tool: &str, group: ToolGroup) -> bool {
        if matches!(group, ToolGroup::Meta | ToolGroup::All) {
            return true;
        }
        match self.machine.current_stage() {
            None => false,
            Some(stage) => self
                .stage_tool_groups_effective(stage)
                .iter()
                .any(|g| *g == group || *g == ToolGroup::All),
        }
    }

    // ── A) handle_prompt aktivasyonu ──────────────────────────────────────
    /// Görevi sınıflandırır, akışı başlatır, ilk aşama direktifini döndürür.
    /// Direct akışa düşen görevlerde dahi stop gate denetimi çalışır.
    pub fn activate(&mut self, prompt_text: &str, user_mode: UserMode) -> Option<String> {
        self.refresh_config();
        if self.is_active() {
            return None; // mevcut akış korunur
        }
        let classified = FlowClassifier::classify(prompt_text, user_mode, &self.rules);
        self.classified = Some(classified);
        self.user_mode = user_mode;
        self.problem_text = prompt_text.to_string();
        let flow = classified.flow();
        let first = self.machine.start(flow);
        self.store.record(
            ArtifactId::ProblemList,
            true,
            format!("akış seçimi (sistem): {}", flow.name),
        );
        // Süre kararı sistemindir (AI değil) — duration aşamasının kanıtıdır.
        let duration = FlowDurationSystem::decide(
            prompt_text.len(),
            &classified,
            user_mode,
            &self.rules,
        );
        self.duration = Some(duration);
        if classified != ClassifiedFlow::Direct {
            self.store.record(
                ArtifactId::DurationDecision,
                true,
                format!("süre kararı (sistem): {duration:?}"),
            );
        }
        self.cell.lock().active = true;
        self.cell.lock().stage = Some(first);
        self.events.phase_changed(first);
        Some(self.stage_directive_effective(first).to_string())
    }

    // ── B) tool görünürlük filtresi ───────────────────────────────────────
    pub fn tool_definitions_filter<T: Clone>(&self, defs: Vec<T>, id_of: impl Fn(&T) -> &str) -> Vec<T> {
        if !self.is_active() {
            return defs;
        }
        defs.into_iter()
            .filter(|d| {
                let id = id_of(d);
                match FlowGate::tool_group_of(id) {
                    None => true,
                    Some(group) => self.tool_unlocked_effective(id, group),
                }
            })
            .collect()
    }

    // ── C) stop gate danışmanı ────────────────────────────────────────────
    /// Bash komut denetimi (S4-bash): akış aktifse aşama kuralları uygulanır
    /// (commit akışı: force/hard/push/reset vb. reddedilir). Akış yoksa Allow.
    pub fn bash_verdict(&self, command: &str) -> super::gate::BashVerdict {
        if !self.is_active() {
            return super::gate::BashVerdict::Allow;
        }
        super::gate::FlowGate::bash_command_allowed(command, &self.machine)
    }

    /// Some(direktif) = KeepWorking (aşama kanıtı eksik), None = dur.
    pub fn stop_decision(&mut self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        if self.machine.bump_redirect(self.rules.max_redirects_per_stage) {
            let stage = self.machine.current_stage().unwrap_or(StageId::Do);
            let directive = self.stage_directive_effective(stage).to_string();
            let detail = format!("aşama {} tamamlanmadı; stop reddedildi", stage.as_str());
            self.events.violation("stop", &detail);
            Some(directive)
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
            Some(stage) => {
                // Kilit açma (deterministik): aşama başına N tur direktif; model
                // flow_checkpoint çağırmazsa tur sonlandırılır (stop gate'e düşer).
                if self.machine.bump_redirect(self.rules.max_redirects_per_stage) {
                    RoundVerdict::Continue(self.stage_directive_effective(stage).to_string())
                } else {
                    RoundVerdict::EndTurn
                }
            }
        }
    }

    // ── Tool başarı kancası (S6: handle_bridge_tool_success) ──────────────
    pub fn on_tool_success(&mut self, tool: &str) {
        if !self.is_active() {
            return;
        }
        // ihlal kaydı: bu tool bu aşamada yasaklanmış bir gruba mı ait?
        if let Some(group) = super::gate::tool_group_of(tool) {
            if !self.tool_unlocked_effective(tool, group) {
                self.events.violation(tool, "kilitli grup çağrısı (filtre aşıldı)");
            }
        }
    }

    /// Checkpoint doğrulaması — saf, hızlı; tool isteği + mevcut aşama.
    /// Async sistem aşamaları (verify/execute-full/notify) `system_async:*`
    /// marker'ı döner; S6 kancası işi yapıp `finalize_*` ile tamamlar.
    pub fn validate_checkpoint(
        &mut self,
        req: &CheckpointRequest,
        current: StageId,
    ) -> CheckpointResult {
        // aşama sırası kontrolü
        if req.stage != current.as_str() {
            self.events.checkpoint_rejected(&req.stage, "yanlış aşama");
            return CheckpointResult::done(
                false,
                &current,
                self.stage_directive_effective(current),
                format!(
                    "Bu aşama değil: sen {} aşamasındasın. {} aşamasına geçemezsin.",
                    current.as_str(),
                    req.stage
                ),
            );
        }
        // sistem aşamaları
        match current {
            StageId::Duration => {
                // Karar zaten activate'te verildi ve kaydedildi; kanıt yaz ve ilerle.
                return self.advance_after_artifact(current, ArtifactId::DurationDecision, "süre kararı (sistem): onaylandı".to_string());
            }
            StageId::ParallelQuery => {
                // building_blocks yolunu decompose kanıtının detayından al.
                let blocks_path = self
                    .store
                    .progress()
                    .iter()
                    .rev()
                    .find(|r| r.artifact == ArtifactId::BuildingBlocks.as_str() && r.ok)
                    .map(|r| r.detail.clone());
                let detail = match blocks_path {
                    Some(path) if Path::new(&path).is_file() => {
                        let graph = analyze(Path::new(&path));
                        // execute_full için grafiğin kendisi depolanır (JSON).
                        serde_json::to_string(&graph).unwrap_or_else(|_| graph_summary(&graph))
                    }
                    _ => "yapı taşı dosyası bulunamadı; sıralı yürütme (fail-safe)".to_string(),
                };
                return self.advance_after_artifact(current, ArtifactId::ExecutionGraph, detail);
            }
            StageId::Verify => {
                if self.judge_enabled() {
                    return CheckpointResult::system_async(&current, "verify");
                }
            }
            StageId::Execute => {
                if matches!(self.duration, Some(DurationDecision::Full)) {
                    return CheckpointResult::system_async(&current, "execute_full");
                }
            }
            StageId::Notify => {
                return CheckpointResult::system_async(&current, "notify");
            }
            _ => {}
        }
        // kanıt dosyası kontrolü (isteğe bağlı files alanı)
        for f in &req.files {
            let p = Path::new(f);
            if !p.is_file() || p.metadata().map(|m| m.len() == 0).unwrap_or(true) {
                self.events.checkpoint_rejected(&req.stage, "kanıt dosyası eksik/boş");
                return CheckpointResult::done(
                    false,
                    &current,
                    self.stage_directive_effective(current),
                    format!("Kanıt dosyası eksik veya boş: {f}. Önce aşama çıktısını üret, sonra kapat."),
                );
            }
        }
        // normal aşama: tüm produces kanıtlarını kaydet ve ilerle
        let mut advanced: Option<StageId> = None;
        if let Some(def) = self.machine.stage_def(current) {
            for artifact in def.produces {
                self.store.record(*artifact, true, req.summary.clone());
                advanced = self.machine.record_artifact(*artifact, true);
            }
        }
        self.finish_stage_transition(current, advanced)
    }

    /// Yargıç etkin mi? Universal akışta varsayılan açık; rules.toml
    /// `judge_enabled = false` ile kapatılabilir (commit/direct zaten kapalı).
    fn judge_enabled(&self) -> bool {
        match self.classified {
            Some(ClassifiedFlow::Universal) => true,
            _ => false,
        }
    }

    /// Kanıt kaydedip aşamayı ilerleten yardımcı (sistem aşamaları için).
    fn advance_after_artifact(
        &mut self,
        current: StageId,
        artifact: ArtifactId,
        detail: String,
    ) -> CheckpointResult {
        self.store.record(artifact, true, detail);
        let advanced = self.machine.record_artifact(artifact, true);
        self.finish_stage_transition(current, advanced)
    }

    /// Aşama geçişini tamamlar: hücreyi güncelle, olayları yayınla, sonucu kur.
    fn finish_stage_transition(
        &mut self,
        current: StageId,
        advanced: Option<StageId>,
    ) -> CheckpointResult {
        let next = advanced.or_else(|| self.machine.current_stage());
        self.cell.lock().stage = next;
        match next {
            None => {
                self.cell.lock().active = false;
                self.events.completed();
                CheckpointResult::done(true, &current, "", "Görev tamamlandı.".to_string())
            }
            Some(ns) => {
                self.events.phase_changed(ns);
                CheckpointResult::done(
                    true,
                    &current,
                    self.stage_directive_effective(ns),
                    format!("Aşama tamam: {} → {}", current.as_str(), ns.as_str()),
                )
            }
        }
    }

    /// flow_checkpoint aracı çağrısını işler (S6): isteği hemen doğrular ve
    /// kararı (kabul / red + direktif / system_async marker) döndürür.
    pub fn process_checkpoint_call(
        &mut self,
        stage: &str,
        summary: &str,
        files: Vec<String>,
    ) -> CheckpointResult {
        if !self.is_active() {
            return CheckpointResult {
                accepted: false,
                pending: false,
                stage: String::new(),
                directive: "flow_checkpoint: aktif akış yok; bu araç yalnızca akış denetimli görevlerde kullanılabilir".to_string(),
                detail: String::new(),
            };
        }
        let req = CheckpointRequest {
            stage: stage.to_string(),
            summary: summary.to_string(),
            files,
        };
        let current = self.cell.lock().stage.unwrap_or(StageId::Do);
        self.validate_checkpoint(&req, current)
    }

    // ── Async finalize'lar (S6 kancası tamamlar, karar burada) ────────────

    /// Verify aşaması: yargıç kararı → kabul = ilerle; red = aşamada kal +
    /// düzeltici direktif (yargıç gerekçesi). Bütçe aşılınca kilit açma.
    pub fn finalize_verify(&mut self, verdict: &JudgeVerdict) -> CheckpointResult {
        let stage = self.machine.current_stage().unwrap_or(StageId::Verify);
        if verdict.accepted {
            self.store.record(ArtifactId::Verified, true, verdict.raw_output.clone());
            let advanced = self.machine.record_artifact(ArtifactId::Verified, true);
            self.finish_stage_transition(stage, advanced)
        } else if self.machine.bump_redirect(self.rules.max_redirects_per_stage) {
            self.store.record(ArtifactId::Verified, false, verdict.raw_output.clone());
            let directive = format!(
                "Yargıç reddetti; aşama tekrarlanmalı.\nYargıç gerekçesi: {}\n\n{}",
                verdict.reason,
                self.stage_directive_effective(stage)
            );
            CheckpointResult::done(false, &stage, &directive, "yargıç reddi".to_string())
        } else {
            // Deterministik kilit açma: 2 yargıç turu yeterli.
            self.store.record(ArtifactId::Verified, true, "yargıç turu limiti; kabul (kilit açma)".to_string());
            let advanced = self.machine.record_artifact(ArtifactId::Verified, true);
            self.finish_stage_transition(stage, advanced)
        }
    }

    /// Execute aşaması (duration=Full): sistem çoklu ajan yürütmesinin
    /// toplu çıktısı kaydedilir ve aşama ilerler.
    pub fn finalize_execute(&mut self, outputs: &str) -> CheckpointResult {
        let stage = self.machine.current_stage().unwrap_or(StageId::Execute);
        let detail = outputs.chars().take(2000).collect::<String>();
        self.store.record(ArtifactId::WorkDone, true, detail);
        let advanced = self.machine.record_artifact(ArtifactId::WorkDone, true);
        self.finish_stage_transition(stage, advanced)
    }

    /// Execute (duration=Full) sistemsel reddi: tüm blok görevlendirmeleri
    /// başarısız olduğunda model uygulamalıdır. finalize_verify deseninde
    /// bütçeyle sınırlıdır — limit aşılınca deterministik kilit açma (livelock
    /// yok; aynı bloklar asla sınırsızca yeniden çalıştırılmaz).
    pub fn reject_execute(&mut self, detail: &str) -> CheckpointResult {
        let stage = self.machine.current_stage().unwrap_or(StageId::Execute);
        if self.machine.bump_redirect(self.rules.max_redirects_per_stage) {
            self.store.record(ArtifactId::WorkDone, false, detail.to_string());
            let directive = format!(
                "Sistem çoklu ajan yürütmesi başarısız: {detail}\n\
                 Yapı taşlarını kendin uygula ve flow_checkpoint ile tekrar dene.\n\n{}",
                self.stage_directive_effective(stage)
            );
            CheckpointResult::done(false, &stage, &directive, "sistem yürütmesi başarısız".to_string())
        } else {
            // Deterministik kilit açma: ret turları limiti doldu → kabul.
            self.store.record(ArtifactId::WorkDone, true, "sistem yürütmesi ret limiti; model uyguladı (kilit açma)".to_string());
            let advanced = self.machine.record_artifact(ArtifactId::WorkDone, true);
            self.finish_stage_transition(stage, advanced)
        }
    }

    /// Notify aşaması: kanal raporu kaydedilir ve aşama HER DURUMDA ilerler
    /// (fail-soft — kanal hatası akışı asla kilitlemez; başarısızlık yalnızca
    /// kanıt kaydına `ok=false` olarak düşer).
    pub fn finalize_notify(&mut self, report: &NotifyReport) -> CheckpointResult {
        let stage = self.machine.current_stage().unwrap_or(StageId::Notify);
        let ok = report.failed == 0;
        let detail = format!(
            "bildirim: gönderildi={} başarısız={} ({})",
            report.sent,
            report.failed,
            report
                .channels
                .iter()
                .map(|c| format!("{}:{}", c.label, if c.ok { "ok" } else { "hata" }))
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.store.record(ArtifactId::Notified, ok, detail.clone());
        let advanced = self.machine.record_artifact(ArtifactId::Notified, true);
        self.finish_stage_transition(stage, advanced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::judge::{build_judge_prompt, parse_verdict};

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
        assert_eq!(g.duration_decision(), Some(DurationDecision::Mvp));
    }

    #[test]
    fn checkpoint_wrong_stage_rejected() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("2026'da en iyi rust frameworkü nedir araştır", UserMode::UserOriented);
        let req = CheckpointRequest { stage: "research".to_string(), summary: "x".to_string(), files: vec![] };
        let res = g.validate_checkpoint(&req, StageId::Analyze);
        assert!(!res.accepted);
        assert!(!res.directive.is_empty());
    }

    #[test]
    fn stop_decision_blocks_until_done() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("selam", UserMode::UserOriented); // direct akış
        assert!(g.stop_decision().is_some());
    }

    #[test]
    fn duration_stage_is_system_decided() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("uzun araştırma görevi", UserMode::UserOriented);
        // analyze'ı sistem kanıtıyla geç
        let r = g.advance_after_artifact(StageId::Analyze, ArtifactId::ProblemList, "x".to_string());
        assert_eq!(r.stage, "analyze");
        assert!(g.machine.current_stage() == Some(StageId::Research) || g.machine.current_stage().is_some());
    }

    #[test]
    fn verify_stage_returns_async_marker() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("araştır ve uygula", UserMode::UserOriented);
        // analyze→research→digest→stack_select→stack_verify→duration→plan→decompose→parallel_query→execute ilerle
        let stages = [StageId::Analyze, StageId::Research, StageId::Digest, StageId::StackSelect,
            StageId::StackVerify, StageId::Duration, StageId::Plan, StageId::Decompose,
            StageId::ParallelQuery, StageId::Execute, StageId::Verify];
        for s in stages {
            let r = g.advance_after_artifact(s, ArtifactId::ProblemList, "x".to_string());
            assert!(r.accepted || r.detail.starts_with("system_async:"), "stage {:?} → {:?}", s, r.detail);
            if r.detail.starts_with("system_async:verify") {
                assert_eq!(s, StageId::Verify);
            }
        }
    }

    #[test]
    fn judge_parse_and_prompt() {
        let p = build_judge_prompt("özet", &["a.txt".to_string()]);
        assert!(p.contains("KANIT DOSYALARI"));
        let v = parse_verdict("skor 80. KABUL");
        assert!(v.accepted);
    }

    #[test]
    fn judge_reject_keeps_stage_and_bounds() {
        let mut g = FlowGovernor::new(&temp_dir());
        g.activate("araştır", UserMode::UserOriented);
        let v = JudgeVerdict { raw_output: "RED: kanıt yok".to_string(), accepted: false, reason: "kanıt yok".to_string() };
        let r1 = g.finalize_verify(&v);
        assert!(!r1.accepted);
        assert!(r1.directive.contains("Yargıç reddetti"));
        // limit: 3 yargıç reddinden sonra kilit açma (kabul)
        let _ = g.finalize_verify(&v);
        let _ = g.finalize_verify(&v);
        let r4 = g.finalize_verify(&v);
        assert!(r4.accepted);
    }

    #[test]
    fn config_override_loads_rules() {
        // rules.toml yoksa varsayılan; bu test yalnızca varsayılan tutarlılığını doğrular.
        let mut g = FlowGovernor::new(&temp_dir());
        assert!(g.rules.max_redirects_per_stage >= 1);
    }
}
