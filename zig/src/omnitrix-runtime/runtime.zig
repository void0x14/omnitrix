//! omnitrix-runtime (tasarım Bölüm 4, 3.2, 3.4, 6).
//!
//! Tek authoritative state Omnitrix runtime içindedir (StateStore).
//! AgentRuntime, Scheduler, Stream, Ledger ve Watchdog bu çatı altında
//! doğrudan typed çağrılarla koordine edilir.

const std = @import("std");

pub const agent = @import("agent.zig");
pub const state_store = @import("state_store.zig");
pub const metrics = @import("metrics.zig");
pub const soak = @import("soak.zig");

pub const AgentStatus = agent.AgentStatus;
pub const AgentStateMachine = agent.AgentStateMachine;
pub const AgentTransitionError = agent.AgentTransitionError;

pub const StateStore = state_store.StateStore;
pub const StateSnapshot = state_store.StateSnapshot;
pub const RuntimeError = state_store.RuntimeError;

pub const TrackedAllocator = metrics.TrackedAllocator;
pub const MemoryMetrics = metrics.MemoryMetrics;
pub const EventLagTracker = metrics.EventLagTracker;
pub const EventLagSnapshot = metrics.EventLagSnapshot;
pub const FirstFrameTracker = metrics.FirstFrameTracker;
pub const ShutdownTracker = metrics.ShutdownTracker;
pub const OsRss = metrics.OsRss;
pub const RuntimeMetricsCollector = metrics.RuntimeMetricsCollector;
pub const MetricsSnapshot = metrics.MetricsSnapshot;

pub const SoakTestHarness = soak.SoakTestHarness;
pub const SoakGateReport = soak.SoakGateReport;
pub const runFullSoakGate = soak.runFullSoakGate;

test {
    std.testing.refAllDecls(@This());
}
