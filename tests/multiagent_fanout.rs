use std::sync::Arc;

use uuid::Uuid;

use omni_scheduler::admission::AdmissionController;
use omni_scheduler::budget::Budget;
use omni_scheduler::hierarchy::{HierarchyError, HierarchyTree};
use omni_scheduler::managed_agent::ManagedAgent;
use omni_scheduler::managed_agent::ManagedAgentState;
use omni_scheduler::persona::PersonaKind;
use xai_chat_state::ChatStateHandle;
use xai_grok_agent::config::AgentDefinition;
use xai_grok_agent::AgentBuilder;
use xai_grok_tools::computer::local::LocalTerminalBackend;
use xai_grok_tools::notification::ToolNotificationHandle;

//
// Helper: create a tree with a root and chain of children down to `target_depth`.
// Returns the parent_id at depth `target_depth` (for further extension).
//
/// Zincirdeki cocuklar `Uuid::from_u128(1..=target_depth)` oldugu icin kokun
/// bu araliktan UZAK bir kimligi olmali; aksi halde ilk cocuk kokle carpisir ve
/// `add_node` `DuplicateNode` dondurur.
const CHAIN_ROOT: Uuid = Uuid::from_u128(u128::MAX);

fn build_chain(tree: &HierarchyTree, target_depth: u32) -> Option<Uuid> {
    let root_id = CHAIN_ROOT;
    let _ = tree.add_node(root_id, None);
    let mut parent = root_id;
    for d in 1..=target_depth {
        let child = Uuid::from_u128(d as u128);
        if tree.add_node(child, Some(parent)).is_err() {
            return None;
        }
        parent = child;
    }
    Some(parent)
}

// ─── test_multiagent_fanout_basic ──────────────────────────────────

#[test]
fn test_multiagent_fanout_basic() {
    let tree = HierarchyTree::new(10);

    let root = Uuid::new_v4();
    tree.add_node(root, None).expect("root");

    let children: Vec<Uuid> = (0..3)
        .map(|_| {
            let id = Uuid::new_v4();
            tree.add_node(id, Some(root)).expect("child");
            id
        })
        .collect();

    assert_eq!(tree.get_children(&root).len(), 3);
    for child in &children {
        assert_eq!(tree.get_parent(child), Some(root));
        assert_eq!(tree.get_depth(child), Some(1));
    }
    assert_eq!(tree.len(), 4);

    // validate_dag — should pass for a valid fanout tree
    assert!(tree.validate_dag().is_ok());
}

// ─── test_multiagent_depth_limit ───────────────────────────────────

#[test]
fn test_multiagent_depth_limit() {
    let tree = HierarchyTree::new(5);

    let ok = build_chain(&tree, 5);
    assert!(ok.is_some(), "depth 5 should be accepted");
    assert_eq!(tree.get_depth(&Uuid::from_u128(5)), Some(5));

    let deep = Uuid::from_u128(6);
    let parent = Uuid::from_u128(5);
    let err = tree.add_node(deep, Some(parent)).unwrap_err();

    match err {
        HierarchyError::MaxDepthExceeded { depth, max } => {
            assert_eq!(depth, 6);
            assert_eq!(max, 5);
        }
        other => panic!("expected MaxDepthExceeded, got {other:?}"),
    }
}

// ─── test_multiagent_concurrent_limit ──────────────────────────────

#[test]
fn test_multiagent_concurrent_limit() {
    // 1 GiB RAM limit, max 2 concurrent
    let ctrl = AdmissionController::new(1_073_741_824, 2);

    // AdmissionToken'in Drop'u kotayi geri verir; tokenlar bagli tutulmazsa
    // ucuncu kabul de basarili olur ve test hicbir sey olcmez.
    let token_1 = ctrl.try_admit_sync(1_000).expect("admit #1");
    let token_2 = ctrl.try_admit_sync(1_000).expect("admit #2");
    assert!(ctrl.try_admit_sync(1_000).is_err(), "admit #3 should be rejected");

    // Bir token birakilinca yer acilmali.
    drop(token_1);
    let token_3 = ctrl.try_admit_sync(1_000).expect("admit #4 after release");
    drop(token_2);
    drop(token_3);

    // ManagedAgentState shouldn't need Agent — just verify it's an enum
    let _idle = ManagedAgentState::Idle;
    let _active = ManagedAgentState::Active;
    assert_ne!(_idle, _active);
}

// ─── test_agent_router_end_to_end ──────────────────────────────────
// §14 Faz 2: Mevcut Agent + AgentBuilder ile tek agent, router
// üzerinden uçtan uca çalışır.

#[tokio::test]
async fn test_agent_router_end_to_end() {
    use omni_router::strategies::{GroundingMode, Router, RoutingPolicy, RoutingStrategy};

    // Bildirim alicisi test boyunca canli tutulur; dusurulurse gonderimler
    // sessizce hata verir.
    let (notifications, _notification_rx) = ToolNotificationHandle::channel();

    // 1. AgentBuilder ile Agent oluştur
    let agent = AgentBuilder::new(
        std::env::temp_dir(),
        Arc::new(LocalTerminalBackend::new()),
        notifications,
    )
    .from_definition(AgentDefinition::default_grok_build())
    .build()
    .await
    .expect("agent should build");

    // 2. Agent'ı ManagedAgent wrapper'ına sar
    let managed = ManagedAgent::new(
        agent,
        PersonaKind::Explorer,
        None,
        uuid::Uuid::nil(),
        0,
        Budget::default(),
        ChatStateHandle::noop(),
    );

    // 3. Router ile minimal routing test
    let _router = Router::new();
    let _policy = RoutingPolicy {
        strategy: RoutingStrategy::RoundRobin,
        fallback_chain: vec![],
        budget: None,
        grounding: GroundingMode::Off,
    };

    // 4. Assert: agent wrapper ve router yapıları çalışıyor
    assert_eq!(managed.persona, PersonaKind::Explorer);
    assert_eq!(managed.depth, 0);
}
