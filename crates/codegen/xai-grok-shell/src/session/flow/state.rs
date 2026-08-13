//! Saf akış durum makinesi. Oturum içi hiçbir duruma bağlanmaz; yalnızca
//! kendi içinde geçişleri yönetir (PlanModeTracker deseni).

use super::definition::{ArtifactId, FlowDefinition, StageDefinition, StageId, ToolGroup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowPhase {
    Idle,
    InStage(StageId),
    Completed,
    Aborted,
}

pub struct FlowStateMachine {
    flow: &'static FlowDefinition,
    phase: FlowPhase,
    stage_index: usize,
    artifact_log: Vec<(ArtifactId, bool)>,
    redirect_count: u32,
}

/// Aşama başına izin verilen varsayılan maksimum düzeltme (KeepWorking) sayısı.
/// Aşılırsa governor deterministik olarak turu bitirir (kilit açma).
/// `rules.toml` `max_redirects_per_stage` ile ezilebilir.
pub const MAX_REDIRECTS_PER_STAGE: u32 = 3;

impl FlowStateMachine {
    pub fn idle() -> Self {
        Self {
            flow: Self::placeholder_flow(),
            phase: FlowPhase::Idle,
            stage_index: 0,
            artifact_log: Vec::new(),
            redirect_count: 0,
        }
    }

    fn placeholder_flow() -> &'static FlowDefinition {
        // Idle iken erişim yok; güvenli bir varsayılan (direct akışı).
        super::definition::find_flow("direct").expect("direct akışı tanımlı olmalı")
    }

    pub fn start(&mut self, flow: &'static FlowDefinition) -> StageId {
        self.flow = flow;
        self.phase = FlowPhase::InStage(flow.stages[0].id);
        self.stage_index = 0;
        self.artifact_log.clear();
        self.redirect_count = 0;
        flow.stages[0].id
    }

    pub fn flow(&self) -> &'static FlowDefinition {
        self.flow
    }

    pub fn phase(&self) -> FlowPhase {
        self.phase
    }

    pub fn current_stage(&self) -> Option<StageId> {
        match self.phase {
            FlowPhase::InStage(id) => Some(id),
            _ => None,
        }
    }

    pub fn current_stage_def(&self) -> Option<&'static StageDefinition> {
        self.current_stage().and_then(|id| self.stage_def(id))
    }

    pub fn stage_def(&self, id: StageId) -> Option<&'static StageDefinition> {
        self.flow.stages.iter().find(|s| s.id == id)
    }

    pub fn stage_tool_groups(&self, stage: StageId) -> &'static [ToolGroup] {
        self.stage_def(stage).map(|s| s.tool_groups).unwrap_or(&[])
    }

    pub fn stage_directive(&self, stage: StageId) -> &'static str {
        self.stage_def(stage).map(|s| s.directive).unwrap_or("")
    }

    /// Kanıt kaydı; aşamanın tüm `produces` kanıtları kayıtlıysa ilerler.
    /// Dönen değer: yeni aşama (ilerlendiyse).
    pub fn record_artifact(&mut self, id: ArtifactId, ok: bool) -> Option<StageId> {
        self.artifact_log.push((id, ok));
        if let Some(stage) = self.current_stage()
            && let Some(def) = self.stage_def(stage)
            && def
                .produces
                .iter()
                .all(|p| self.artifact_log.iter().any(|(a, k)| a == p && *k))
        {
            return self.try_advance();
        }
        None
    }

    /// Bir sonraki aşamaya geçer; son aşamaysa Completed yapar.
    pub fn try_advance(&mut self) -> Option<StageId> {
        self.redirect_count = 0;
        let next = self.stage_index + 1;
        if next < self.flow.stages.len() {
            self.stage_index = next;
            let id = self.flow.stages[next].id;
            self.phase = FlowPhase::InStage(id);
            Some(id)
        } else {
            self.phase = FlowPhase::Completed;
            None
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.phase, FlowPhase::Completed)
    }

    /// Aşama kilitli mi? `meta` ve `All` her zaman açıktır.
    pub fn tool_unlocked(&self, group: ToolGroup) -> bool {
        if matches!(group, ToolGroup::Meta | ToolGroup::All) {
            return true;
        }
        match self.current_stage() {
            None => false,
            Some(stage) => {
                self.stage_tool_groups(stage).contains(&group)
                    || self.stage_tool_groups(stage).contains(&ToolGroup::All)
            }
        }
    }

    /// Düzeltme sayacı; limit (aşama başına) aşıldıysa false (governor turu bitirir).
    pub fn bump_redirect(&mut self, max: u32) -> bool {
        self.redirect_count += 1;
        self.redirect_count <= max.max(1)
    }

    pub fn redirect_count(&self) -> u32 {
        self.redirect_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::definition::default_flows;

    #[test]
    fn universal_flow_advances_in_order() {
        let flows = default_flows();
        let universal = flows.iter().find(|f| f.name == "universal").unwrap();
        let mut sm = FlowStateMachine::idle();
        let first = sm.start(universal);
        assert_eq!(first, StageId::Analyze);
        // her aşama produces kanıtını kaydet
        let mut stage = sm.current_stage();
        while let Some(s) = stage {
            let def = sm.stage_def(s).unwrap();
            for p in def.produces {
                sm.record_artifact(*p, true);
            }
            stage = sm.current_stage();
        }
        assert!(sm.is_complete());
    }

    #[test]
    fn meta_group_always_unlocked() {
        let flows = default_flows();
        let universal = flows.iter().find(|f| f.name == "universal").unwrap();
        let mut sm = FlowStateMachine::idle();
        sm.start(universal);
        assert!(sm.tool_unlocked(ToolGroup::Meta));
        assert!(!sm.tool_unlocked(ToolGroup::Web)); // research aşaması kilitli
    }
}
