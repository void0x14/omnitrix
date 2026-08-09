//! Problem süre sistemi: MVP mi TAM mı kararını AI DEĞİL, bu saf sistem verir.

use super::classifier::{ClassifiedFlow, FlowRules, UserMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationDecision {
    Mvp,
    Full,
}

pub struct FlowDurationSystem;

impl FlowDurationSystem {
    pub fn decide(task_len: usize, classified: &ClassifiedFlow, mode: UserMode, rules: &FlowRules) -> DurationDecision {
        match classified {
            ClassifiedFlow::Commit | ClassifiedFlow::Direct => DurationDecision::Mvp,
            ClassifiedFlow::Universal => {
                if matches!(mode, UserMode::Autonomous) {
                    return DurationDecision::Full;
                }
                if task_len <= rules.mvp_max_len {
                    DurationDecision::Mvp
                } else {
                    DurationDecision::Full
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::classifier::default_rules;

    #[test]
    fn short_task_is_mvp() {
        let r = default_rules();
        let d = FlowDurationSystem::decide(50, &ClassifiedFlow::Universal, UserMode::UserOriented, &r);
        assert_eq!(d, DurationDecision::Mvp);
    }

    #[test]
    fn autonomous_mode_is_full() {
        let r = default_rules();
        let d = FlowDurationSystem::decide(50, &ClassifiedFlow::Universal, UserMode::Autonomous, &r);
        assert_eq!(d, DurationDecision::Full);
    }
}
