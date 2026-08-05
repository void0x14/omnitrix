//! Omnitrix bridge — the seam through which the omnitrix core (running in the
//! `omnitrix` binary) exposes itself to the pager TUI.
//!
//! Two once-only slots:
//!
//! - [`OmniSnapshotProvider`]: installed by the omnitrix binary after warm-up;
//!   `/omni` reads a [`OmniSnapshot`] through it.
//! - [`OmniEventSink`]: the reverse direction — pager-side event hooks that the
//!   omnitrix core can call into (ACP messages, tool calls, prompts). Task 1.3
//!   wires actual consumers; this module only defines the seam.

use std::sync::Arc;
use std::sync::OnceLock;

/// Point-in-time health/size summary of the omnitrix core.
#[derive(Clone, Copy, Debug)]
pub struct OmniSnapshot {
    /// Number of configured providers.
    pub providers: usize,
    /// Number of scheduler-active agents right now.
    pub active_agents: usize,
    /// Storage footprint in bytes (CAS directory size).
    pub storage_bytes: u64,
    /// Whether the core's health probe reports healthy.
    pub healthy: bool,
}

/// Provider of an omnitrix core snapshot. Implemented by the omnitrix binary
/// over its own context (scheduler, storage, provider layer, health probe).
pub trait OmniSnapshotProvider: Send + Sync {
    /// Produce the current snapshot. Must be cheap and non-blocking.
    fn snapshot(&self) -> OmniSnapshot;
}

static SNAPSHOT_PROVIDER: OnceLock<Arc<dyn OmniSnapshotProvider>> = OnceLock::new();

/// Install the omnitrix core snapshot provider. First call wins; a second
/// install (e.g. a second core instance) is rejected with `Err(())`.
pub fn install(p: Arc<dyn OmniSnapshotProvider>) -> Result<(), ()> {
    SNAPSHOT_PROVIDER.set(p).map_err(|_| ())
}

/// Take a snapshot from the installed provider, if any.
///
/// `None` means the omnitrix core has not installed a provider yet (warm-up
/// pending or the pager is running standalone).
pub fn snapshot() -> Option<OmniSnapshot> {
    SNAPSHOT_PROVIDER.get().map(|p| p.snapshot())
}

/// Pager-side event hooks the omnitrix core can call into. Task 1.3 installs
/// real consumers; the seam itself is installed here.
pub trait OmniEventSink: Send + Sync {
    /// A raw ACP wire message (JSON) crossed the pager/shell boundary.
    fn on_acp_message(&self, json: &str);
    /// An agent tool call was observed.
    fn on_tool_call(&self, name: &str, args: &str);
    /// A user prompt was submitted.
    fn on_prompt(&self, text: &str);
}

static EVENT_SINK: OnceLock<Arc<dyn OmniEventSink>> = OnceLock::new();

/// Install the pager-side event sink. First call wins; later installs are
/// silently ignored (the first sink stays authoritative).
pub fn install_sink(sink: Arc<dyn OmniEventSink>) {
    let _ = EVENT_SINK.set(sink);
}

/// The installed event sink, if any.
pub fn event_sink() -> Option<Arc<dyn OmniEventSink>> {
    EVENT_SINK.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider(OmniSnapshot);

    impl OmniSnapshotProvider for FakeProvider {
        fn snapshot(&self) -> OmniSnapshot {
            self.0
        }
    }

    /// OnceLock is process-global: any earlier test may have installed a
    /// provider, so assertions must hold under both orderings. First-wins is
    /// verified in whichever branch runs: a successful first install must
    /// reject the second and be readable; an already-installed slot must
    /// reject both and still serve a snapshot.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn first_install_wins_second_rejected() {
        let first = Arc::new(FakeProvider(OmniSnapshot {
            providers: 1,
            active_agents: 2,
            storage_bytes: 1024,
            healthy: true,
        }));
        let second = Arc::new(FakeProvider(OmniSnapshot {
            providers: 9,
            active_agents: 9,
            storage_bytes: 999,
            healthy: false,
        }));
        if install(first).is_ok() {
            assert!(install(second).is_err(), "second install must lose");
            let snap = snapshot().expect("provider installed");
            assert_eq!(snap.providers, 1);
            assert_eq!(snap.active_agents, 2);
            assert!(snap.healthy);
        } else {
            assert!(install(second).is_err(), "slot already taken");
            assert!(snapshot().is_some(), "an earlier test installed a provider");
        }
    }

    struct FakeSink;

    impl OmniEventSink for FakeSink {
        fn on_acp_message(&self, _json: &str) {}
        fn on_tool_call(&self, _name: &str, _args: &str) {}
        fn on_prompt(&self, _text: &str) {}
    }

    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn event_sink_retrievable_after_install() {
        install_sink(Arc::new(FakeSink));
        assert!(event_sink().is_some());
    }
}
