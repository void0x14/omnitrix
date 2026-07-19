#![cfg(feature = "ork")]

/// Initialize the `ork-runtime` subsystem.
///
/// Called once during workspace server startup when the `ork` feature is
/// enabled.  Currently a no-op placeholder for the multiagent orchestration
/// runtime.  Actual `ork-runtime` wiring will be added in a follow-up phase.
pub fn init() {
    tracing::info!("ork-runtime available (feature-gated)");
}
