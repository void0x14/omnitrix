use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::hierarchy::HierarchyTree;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterruptLevel {
    ContextWarning,
    TaskStop,
    AgentKill,
    TrustDegrade,
    Quarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterruptAction {
    FeedbackInject,
    Retry,
    TaskCancel,
    Replan,
    AgentKill,
    ResourceReclaim,
    TrustScoreDown,
    ProviderDisable,
}

impl InterruptLevel {
    pub fn severity(&self) -> u8 {
        match self {
            Self::ContextWarning => 1,
            Self::TaskStop => 2,
            Self::AgentKill => 3,
            Self::TrustDegrade => 4,
            Self::Quarantine => 5,
        }
    }

    pub fn is_hard(&self) -> bool {
        matches!(self, Self::AgentKill | Self::Quarantine)
    }

    pub fn default_actions(&self) -> Vec<InterruptAction> {
        match self {
            Self::ContextWarning => vec![InterruptAction::FeedbackInject, InterruptAction::Retry],
            Self::TaskStop => vec![InterruptAction::TaskCancel, InterruptAction::Replan],
            Self::AgentKill => vec![InterruptAction::AgentKill, InterruptAction::ResourceReclaim],
            Self::TrustDegrade => vec![InterruptAction::TrustScoreDown],
            Self::Quarantine => vec![InterruptAction::ProviderDisable],
        }
    }

    pub fn trust_delta(&self) -> f64 {
        match self {
            Self::ContextWarning => -0.02,
            Self::TaskStop => -0.05,
            Self::AgentKill => -0.10,
            Self::TrustDegrade => -0.20,
            Self::Quarantine => -0.40,
        }
    }

    pub fn propagates_to_children(&self) -> bool {
        match self {
            Self::ContextWarning => false,
            Self::TaskStop => true,
            Self::AgentKill => true,
            Self::TrustDegrade => true,
            Self::Quarantine => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interrupt {
    pub id: Uuid,
    pub target_agent_id: Uuid,
    pub level: InterruptLevel,
    pub reason: String,
    pub issued_at: chrono::DateTime<chrono::Utc>,
    pub issued_by: Option<Uuid>,
    pub is_broadcast: bool,
}

impl Interrupt {
    pub fn targets_all(&self) -> bool {
        self.is_broadcast
    }

    pub fn targets(&self, agent_id: &Uuid) -> bool {
        self.is_broadcast || self.target_agent_id == *agent_id
    }
}

#[derive(Clone)]
pub struct InterruptBus {
    tx: broadcast::Sender<Interrupt>,
    hierarchy: Option<Arc<HierarchyTree>>,
}

impl InterruptBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, hierarchy: None }
    }

    pub fn with_hierarchy(capacity: usize, hierarchy: Arc<HierarchyTree>) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, hierarchy: Some(hierarchy) }
    }

    pub fn set_hierarchy(&mut self, hierarchy: Arc<HierarchyTree>) {
        self.hierarchy = Some(hierarchy);
    }

    pub fn send(&self, interrupt: Interrupt) -> Result<(), broadcast::error::SendError<Interrupt>> {
        self.tx.send(interrupt.clone())?;

        if let Some(ref hierarchy) = self.hierarchy
            && interrupt.level.propagates_to_children()
            && !interrupt.is_broadcast
        {
            self.propagate_to_children(hierarchy, &interrupt);
        }

        Ok(())
    }

    fn propagate_to_children(&self, hierarchy: &HierarchyTree, interrupt: &Interrupt) {
        let children = hierarchy.get_children(&interrupt.target_agent_id);
        for child_id in children {
            let child_interrupt = Interrupt {
                id: Uuid::new_v4(),
                target_agent_id: child_id,
                level: interrupt.level,
                reason: format!("propagated from parent {}: {}", interrupt.target_agent_id, interrupt.reason),
                issued_at: chrono::Utc::now(),
                issued_by: Some(interrupt.target_agent_id),
                is_broadcast: false,
            };
            let _ = self.tx.send(child_interrupt.clone());
            self.propagate_to_children(hierarchy, &Interrupt {
                target_agent_id: child_id,
                level: interrupt.level,
                reason: interrupt.reason.clone(),
                ..interrupt.clone()
            });
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Interrupt> {
        self.tx.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn broadcast_all(&self, level: InterruptLevel, reason: impl Into<String>, by: Option<Uuid>) {
        let interrupt = Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: Uuid::nil(),
            level,
            reason: format!("[BROADCAST] {}", reason.into()),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: true,
        };
        let _ = self.tx.send(interrupt);
    }

    pub fn send_warning(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::ContextWarning,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_task_stop(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::TaskStop,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_kill(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::AgentKill,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_trust_degrade(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::TrustDegrade,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_quarantine(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::Quarantine,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }
}

impl Default for InterruptBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_level_severity_ordering() {
        assert!(InterruptLevel::ContextWarning.severity() < InterruptLevel::TaskStop.severity());
        assert!(InterruptLevel::TaskStop.severity() < InterruptLevel::AgentKill.severity());
        assert!(InterruptLevel::AgentKill.severity() < InterruptLevel::TrustDegrade.severity());
        assert!(InterruptLevel::TrustDegrade.severity() < InterruptLevel::Quarantine.severity());
        assert_eq!(InterruptLevel::Quarantine.severity(), 5);
    }

    #[test]
    fn test_default_actions_per_level() {
        assert_eq!(
            InterruptLevel::ContextWarning.default_actions(),
            vec![InterruptAction::FeedbackInject, InterruptAction::Retry]
        );
        assert_eq!(
            InterruptLevel::TaskStop.default_actions(),
            vec![InterruptAction::TaskCancel, InterruptAction::Replan]
        );
        assert_eq!(
            InterruptLevel::AgentKill.default_actions(),
            vec![InterruptAction::AgentKill, InterruptAction::ResourceReclaim]
        );
        assert_eq!(
            InterruptLevel::TrustDegrade.default_actions(),
            vec![InterruptAction::TrustScoreDown]
        );
        assert_eq!(
            InterruptLevel::Quarantine.default_actions(),
            vec![InterruptAction::ProviderDisable]
        );
    }

    #[test]
    fn test_trust_delta_values() {
        assert!((InterruptLevel::TrustDegrade.trust_delta() - (-0.20)).abs() < f64::EPSILON);
        assert!((InterruptLevel::Quarantine.trust_delta() - (-0.40)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_propagation_rules() {
        assert!(!InterruptLevel::ContextWarning.propagates_to_children());
        assert!(InterruptLevel::TaskStop.propagates_to_children());
        assert!(InterruptLevel::AgentKill.propagates_to_children());
        assert!(InterruptLevel::TrustDegrade.propagates_to_children());
        assert!(InterruptLevel::Quarantine.propagates_to_children());
    }

    #[test]
    fn test_send_and_receive() {
        let bus = InterruptBus::new(16);
        let mut rx = bus.subscribe();

        bus.send_warning(Uuid::new_v4(), "test warning", None);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.level, InterruptLevel::ContextWarning);
        assert!(!received.is_broadcast);
    }

    #[test]
    fn test_broadcast_all() {
        let bus = InterruptBus::new(16);
        let mut rx = bus.subscribe();

        bus.broadcast_all(InterruptLevel::TaskStop, "all stop", None);

        let received = rx.try_recv().unwrap();
        assert!(received.is_broadcast);
        assert!(received.targets_all());
        assert_eq!(received.level, InterruptLevel::TaskStop);
    }

    #[test]
    fn test_target_matching() {
        let agent_id = Uuid::new_v4();
        let interrupt = Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: agent_id,
            level: InterruptLevel::TaskStop,
            reason: "test".into(),
            issued_at: chrono::Utc::now(),
            issued_by: None,
            is_broadcast: false,
        };

        assert!(interrupt.targets(&agent_id));
        assert!(!interrupt.targets(&Uuid::new_v4()));

        let broadcast = Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: Uuid::nil(),
            level: InterruptLevel::Quarantine,
            reason: "all".into(),
            issued_at: chrono::Utc::now(),
            issued_by: None,
            is_broadcast: true,
        };

        assert!(broadcast.targets(&Uuid::new_v4()));
        assert!(broadcast.targets_all());
    }

    #[test]
    fn test_child_propagation() {
        let hierarchy = Arc::new(HierarchyTree::new(5));
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        hierarchy.add_node(parent, None).unwrap();
        hierarchy.add_node(child, Some(parent)).unwrap();

        let bus = InterruptBus::with_hierarchy(16, Arc::clone(&hierarchy));
        let mut rx = bus.subscribe();

        bus.send_kill(parent, "kill parent", None);

        let first = rx.try_recv().unwrap();
        assert_eq!(first.target_agent_id, parent);

        let second = rx.try_recv().unwrap();
        assert_eq!(second.target_agent_id, child);
        assert!(second.reason.contains("propagated from parent"));
    }

    #[test]
    fn test_context_warning_no_propagation() {
        let hierarchy = Arc::new(HierarchyTree::new(5));
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        hierarchy.add_node(parent, None).unwrap();
        hierarchy.add_node(child, Some(parent)).unwrap();

        let bus = InterruptBus::with_hierarchy(16, Arc::clone(&hierarchy));
        let mut rx = bus.subscribe();

        bus.send_warning(parent, "soft warning", None);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.target_agent_id, parent);
        assert!(rx.try_recv().is_err());
    }
}
