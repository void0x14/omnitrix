//! Agent Durum Makinesi (tasarım Bölüm 3.2, 4).
//!
//! Ajanın yaşam döngüsü durumları ve geçiş kuralları:
//! idle, thinking, executing_tool, streaming_response, awaiting_input, terminated, failed.
//!
//! Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");

pub const AgentStatus = enum {
    idle,
    thinking,
    executing_tool,
    streaming_response,
    awaiting_input,
    terminated,
    failed,

    pub fn label(self: AgentStatus) []const u8 {
        return @tagName(self);
    }

    pub fn isBusy(self: AgentStatus) bool {
        return self == .thinking or self == .executing_tool or self == .streaming_response;
    }
};

pub const AgentTransitionError = error{
    InvalidAgentTransition,
    AgentAlreadyTerminated,
};

pub fn canTransitionAgent(from: AgentStatus, to: AgentStatus) bool {
    if (from == .terminated) return false;
    if (to == .terminated) return true; // Her durumdan sonlandırmaya geçilebilir
    if (to == .failed) return true; // Hata durumuna her aktif adımdan geçilebilir

    return switch (from) {
        .idle => to == .thinking or to == .awaiting_input,
        .thinking => to == .executing_tool or to == .streaming_response or to == .awaiting_input or to == .idle,
        .executing_tool => to == .thinking or to == .streaming_response or to == .idle,
        .streaming_response => to == .thinking or to == .awaiting_input or to == .idle,
        .awaiting_input => to == .thinking or to == .idle,
        .failed => to == .idle or to == .thinking, // Hata sonrası toparlanma (recovery)
        .terminated => false,
    };
}

pub const AgentStateMachine = struct {
    status: AgentStatus = .idle,
    current_task_id: ?u64 = null,
    active_tool_name: ?[]const u8 = null,
    last_error: ?[]const u8 = null,

    pub fn init() AgentStateMachine {
        return .{};
    }

    pub fn transition(self: *AgentStateMachine, to: AgentStatus) AgentTransitionError!void {
        if (self.status == .terminated) return error.AgentAlreadyTerminated;
        if (!canTransitionAgent(self.status, to)) return error.InvalidAgentTransition;
        self.status = to;
    }

    pub fn startThinking(self: *AgentStateMachine, task_id: u64) AgentTransitionError!void {
        try self.transition(.thinking);
        self.current_task_id = task_id;
        self.active_tool_name = null;
        self.last_error = null;
    }

    pub fn startToolExecution(self: *AgentStateMachine, tool_name: []const u8) AgentTransitionError!void {
        try self.transition(.executing_tool);
        self.active_tool_name = tool_name;
    }

    pub fn startStreaming(self: *AgentStateMachine) AgentTransitionError!void {
        try self.transition(.streaming_response);
        self.active_tool_name = null;
    }

    pub fn awaitInput(self: *AgentStateMachine) AgentTransitionError!void {
        try self.transition(.awaiting_input);
        self.active_tool_name = null;
    }

    pub fn setIdle(self: *AgentStateMachine) AgentTransitionError!void {
        try self.transition(.idle);
        self.current_task_id = null;
        self.active_tool_name = null;
    }

    pub fn fail(self: *AgentStateMachine, err_msg: []const u8) void {
        self.last_error = err_msg;
        _ = self.transition(.failed) catch {};
    }

    pub fn terminate(self: *AgentStateMachine) void {
        _ = self.transition(.terminated) catch {};
    }
};

test "agent state machine standart akis" {
    var sm = AgentStateMachine.init();
    try std.testing.expectEqual(AgentStatus.idle, sm.status);

    try sm.startThinking(101);
    try std.testing.expectEqual(AgentStatus.thinking, sm.status);
    try std.testing.expectEqual(@as(u64, 101), sm.current_task_id.?);

    try sm.startToolExecution("file_write");
    try std.testing.expectEqual(AgentStatus.executing_tool, sm.status);
    try std.testing.expectEqualStrings("file_write", sm.active_tool_name.?);

    try sm.startStreaming();
    try std.testing.expectEqual(AgentStatus.streaming_response, sm.status);

    try sm.awaitInput();
    try std.testing.expectEqual(AgentStatus.awaiting_input, sm.status);

    try sm.setIdle();
    try std.testing.expectEqual(AgentStatus.idle, sm.status);
    try std.testing.expect(sm.current_task_id == null);
}

test "agent gecersiz durum gecisi reddedilir" {
    var sm = AgentStateMachine.init();
    // idle durumundan doğrudan executing_tool yapılamaz (önce thinking olmalı)
    try std.testing.expectError(error.InvalidAgentTransition, sm.startToolExecution("tool"));
}

test "agent terminate sonrasi gecis reddedilir" {
    var sm = AgentStateMachine.init();
    sm.terminate();
    try std.testing.expectEqual(AgentStatus.terminated, sm.status);
    try std.testing.expectError(error.AgentAlreadyTerminated, sm.startThinking(1));
}
