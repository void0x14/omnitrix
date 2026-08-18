//! omnitrix-permission (tasarım Bölüm 3.2, 7 Kol C - C1).
//!
//! Tool broker, capability kuralları, deny -> gizleme, doom-loop circuit-breaker.
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");

pub const capability = @import("capability.zig");
pub const doom_loop = @import("doom_loop.zig");
pub const broker = @import("broker.zig");

pub const CapabilityAction = capability.CapabilityAction;
pub const CapabilityRule = capability.CapabilityRule;
pub const CapabilityDecision = capability.CapabilityDecision;
pub const CapabilityEngine = capability.CapabilityEngine;

pub const DoomLoopError = doom_loop.DoomLoopError;
pub const DoomLoopConfig = doom_loop.DoomLoopConfig;
pub const DoomLoopDetector = doom_loop.DoomLoopDetector;

pub const OperationContext = broker.OperationContext;
pub const ToolDef = broker.ToolDef;
pub const ToolResult = broker.ToolResult;
pub const ToolHandler = broker.ToolHandler;
pub const ToolBroker = broker.ToolBroker;
pub const BrokerError = broker.BrokerError;

test {
    std.testing.refAllDecls(@This());
}
