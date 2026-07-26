use omni_scheduler::admission::{AdmissionController, AdmissionError, ContextSwapManager};
use omni_storage::cas::CasBlobStore;

#[test]
fn test_admission_ram_backpressure() {
    let ctrl = AdmissionController::new(1_000, 10);

    assert!(ctrl.ram_pressure() < 0.01);

    let token = ctrl.try_admit_sync(800).expect("admit 800");
    let p = ctrl.ram_pressure();
    assert!(p > 0.79 && p < 0.81, "pressure={p}");

    match ctrl.try_admit_sync(800) {
        Err(AdmissionError::RamExceeded { current, requested, max }) => {
            assert_eq!(current, 800);
            assert_eq!(requested, 800);
            assert_eq!(max, 1_000);
        }
        Err(e) => panic!("expected RamExceeded, got {e:?}"),
        Ok(_) => panic!("expected RamExceeded, got Ok"),
    }

    drop(token);
    assert!(ctrl.ram_pressure() < 0.01);
}

#[test]
fn test_context_swap_out_restore() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cas = CasBlobStore::new(dir.path()).expect("CAS");
    let swap = ContextSwapManager::new(cas, 60_000);

    let agent_id = "agent-42";
    let context = "agent context data here";

    let hash = swap.swap_out(agent_id, context).expect("swap out");
    assert!(!hash.is_empty());

    let restored = swap.swap_in(agent_id).expect("swap in");
    assert_eq!(restored.as_deref(), Some(context));

    swap.evict_agent(agent_id);
    let after_evict = swap.swap_in(agent_id).expect("swap in after evict");
    assert!(after_evict.is_none());
}

#[test]
fn test_active_vs_existing_agent_count() {
    let ctrl = AdmissionController::new(1 << 40, 3);

    let mut tokens = Vec::new();

    for i in 0..3 {
        let t = ctrl
            .try_admit_sync(1_000)
            .unwrap_or_else(|_| panic!("admit #{i}"));
        tokens.push(t);
    }

    match ctrl.try_admit_sync(1_000) {
        Err(AdmissionError::ConcurrencyLimit) => {}
        Err(e) => panic!("expected ConcurrencyLimit, got {e:?}"),
        Ok(_) => panic!("expected ConcurrencyLimit, got Ok"),
    }

    tokens.pop();
    let _t4 = ctrl.try_admit_sync(1_000).expect("admit after drop");
}
