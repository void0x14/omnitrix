//! omnitrix-permission: Tool broker ve operasyon yönetimi (tasarım Bölüm 3.2, 7 Kol C - C1).
//!
//! Tool broker, model ile sistem araçları arasındaki kontrol kapısıdır:
//! 1. CapabilityEngine üzerinden allow/deny/ask_user kontrolleri
//! 2. Deny -> gizleme: Yasaklı tool'lar model şemalarından gizlenir (getVisibleTools)
//! 3. DoomLoopDetector üzerinden döngü ve circuit breaker koruması
//! 4. OperationContext sahiplik bağlamı (operation_id, agent_id, session_id, turn_id)
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const capability = @import("capability.zig");
const doom_loop = @import("doom_loop.zig");

pub const CapabilityEngine = capability.CapabilityEngine;
pub const CapabilityAction = capability.CapabilityAction;
pub const DoomLoopDetector = doom_loop.DoomLoopDetector;
pub const DoomLoopConfig = doom_loop.DoomLoopConfig;

pub const BrokerError = doom_loop.DoomLoopError || error{
    ToolNotFound,
    ToolDenied,
    ToolHidden,
    UserConfirmationRequired,
    ToolExecutionFailed,
    CircuitBreakerTripped,
    DoomLoopDetected,
} || std.mem.Allocator.Error;

/// Operasyon sahipliği bağlamı (tasarım Bölüm 6.0, 6.1).
pub const OperationContext = struct {
    operation_id: []const u8,
    agent_id: []const u8,
    session_id: []const u8,
    turn_id: u64,
};

/// Tool tanımı (şema).
pub const ToolDef = struct {
    name: []const u8,
    description: []const u8,
    schema_json: []const u8,

    pub fn clone(self: ToolDef, allocator: std.mem.Allocator) !ToolDef {
        return .{
            .name = try allocator.dupe(u8, self.name),
            .description = try allocator.dupe(u8, self.description),
            .schema_json = try allocator.dupe(u8, self.schema_json),
        };
    }

    pub fn deinit(self: *ToolDef, allocator: std.mem.Allocator) void {
        allocator.free(self.name);
        allocator.free(self.description);
        allocator.free(self.schema_json);
        self.* = undefined;
    }
};

pub const ToolResult = struct {
    output: []const u8,
    is_error: bool = false,

    pub fn deinit(self: *ToolResult, allocator: std.mem.Allocator) void {
        allocator.free(self.output);
        self.* = undefined;
    }
};

pub const ToolHandler = *const fn (
    allocator: std.mem.Allocator,
    args_json: []const u8,
    ctx: OperationContext,
) anyerror!ToolResult;

const RegisteredTool = struct {
    def: ToolDef,
    handler: ToolHandler,
};

pub const ToolBroker = struct {
    allocator: std.mem.Allocator,
    capability_engine: CapabilityEngine,
    doom_loop_detector: DoomLoopDetector,
    tools: std.StringHashMap(RegisteredTool),

    pub fn init(
        allocator: std.mem.Allocator,
        default_action: CapabilityAction,
        doom_loop_config: DoomLoopConfig,
    ) ToolBroker {
        return .{
            .allocator = allocator,
            .capability_engine = CapabilityEngine.init(allocator, default_action),
            .doom_loop_detector = DoomLoopDetector.init(allocator, doom_loop_config),
            .tools = std.StringHashMap(RegisteredTool).init(allocator),
        };
    }

    pub fn deinit(self: *ToolBroker) void {
        var it = self.tools.valueIterator();
        while (it.next()) |tool| {
            tool.def.deinit(self.allocator);
        }
        self.tools.deinit();
        self.doom_loop_detector.deinit();
        self.capability_engine.deinit();
        self.* = undefined;
    }

    /// Tool kaydeder.
    pub fn registerTool(
        self: *ToolBroker,
        def: ToolDef,
        handler: ToolHandler,
    ) !void {
        const owned_def = try def.clone(self.allocator);
        errdefer {
            var mut_def = owned_def;
            mut_def.deinit(self.allocator);
        }

        try self.tools.put(owned_def.name, .{
            .def = owned_def,
            .handler = handler,
        });
    }

    /// Deny -> gizleme kuralı: Modele veya istemciye sunulacak tool listesini
    /// döndürür. Yasaklanmış (deny) tool'lar bu listeden tamamen elenir (gizlenir).
    pub fn getVisibleTools(self: *const ToolBroker, allocator: std.mem.Allocator) !std.ArrayList(ToolDef) {
        var visible = std.ArrayList(ToolDef).empty;
        errdefer {
            for (visible.items) |*t| t.deinit(allocator);
            visible.deinit(allocator);
        }

        var it = self.tools.valueIterator();
        while (it.next()) |reg| {
            if (self.capability_engine.isToolVisible(reg.def.name)) {
                const cloned = try reg.def.clone(allocator);
                try visible.append(allocator, cloned);
            }
        }

        return visible;
    }

    /// Tool çalıştırma kapısı.
    pub fn execute(
        self: *ToolBroker,
        tool_name: []const u8,
        args_json: []const u8,
        ctx: OperationContext,
        user_confirmed: bool,
    ) BrokerError!ToolResult {
        const reg = self.tools.get(tool_name) orelse return error.ToolNotFound;

        // 1. Yetki kontrolü (Capability check)
        const decision = self.capability_engine.evaluate(tool_name);
        switch (decision.action) {
            .deny => return error.ToolDenied,
            .ask_user => {
                if (!user_confirmed) {
                    return error.UserConfirmationRequired;
                }
            },
            .allow => {},
        }

        // 2. Doom loop kontrolü (Call signature check)
        self.doom_loop_detector.recordCall(tool_name, args_json) catch |err| switch (err) {
            error.DoomLoopDetected => return error.DoomLoopDetected,
            error.CircuitBreakerTripped => return error.CircuitBreakerTripped,
            else => return error.ToolExecutionFailed,
        };

        // 3. Tool handler'ı çalıştır
        const result = reg.handler(self.allocator, args_json, ctx) catch |err| {
            const err_name = @errorName(err);
            self.doom_loop_detector.recordError(tool_name, err_name) catch {};
            return error.ToolExecutionFailed;
        };

        if (result.is_error) {
            self.doom_loop_detector.recordError(tool_name, result.output) catch {};
        } else {
            self.doom_loop_detector.recordSuccess();
        }

        return result;
    }
};

fn dummyEchoHandler(allocator: std.mem.Allocator, args_json: []const u8, ctx: OperationContext) anyerror!ToolResult {
    _ = ctx;
    const out = try std.fmt.allocPrint(allocator, "echo: {s}", .{args_json});
    return .{ .output = out, .is_error = false };
}

fn dummyFailingHandler(allocator: std.mem.Allocator, args_json: []const u8, ctx: OperationContext) anyerror!ToolResult {
    _ = ctx;
    _ = args_json;
    _ = allocator;
    return error.FileNotFound;
}

test "tool broker registration ve deny -> gizleme" {
    var broker = ToolBroker.init(std.testing.allocator, .allow, .{});
    defer broker.deinit();

    try broker.registerTool(.{
        .name = "read_file",
        .description = "Dosya okur",
        .schema_json = "{}",
    }, dummyEchoHandler);

    try broker.registerTool(.{
        .name = "danger_rm",
        .description = "Silme yapar",
        .schema_json = "{}",
    }, dummyEchoHandler);

    try broker.capability_engine.addRule("danger_*", .deny, "forbidden");

    // Visible tools listesini al -> danger_rm gizlenmiş olmalı
    var visible = try broker.getVisibleTools(std.testing.allocator);
    defer {
        for (visible.items) |*t| t.deinit(std.testing.allocator);
        visible.deinit(std.testing.allocator);
    }

    try std.testing.expectEqual(@as(usize, 1), visible.items.len);
    try std.testing.expectEqualStrings("read_file", visible.items[0].name);
}

test "tool broker execution allow / deny / ask_user / doom-loop" {
    var broker = ToolBroker.init(std.testing.allocator, .ask_user, .{
        .max_identical_calls = 2,
    });
    defer broker.deinit();

    try broker.registerTool(.{
        .name = "read_file",
        .description = "Dosya okur",
        .schema_json = "{}",
    }, dummyEchoHandler);

    try broker.registerTool(.{
        .name = "failing_tool",
        .description = "Hata verir",
        .schema_json = "{}",
    }, dummyFailingHandler);

    try broker.capability_engine.addRule("read_file", .allow, null);
    try broker.capability_engine.addRule("failing_tool", .allow, null);

    const ctx = OperationContext{
        .operation_id = "op_1",
        .agent_id = "agent_juryrigg",
        .session_id = "sess_test",
        .turn_id = 1,
    };

    // 1. Allow -> başarılı çalışır
    var res1 = try broker.execute("read_file", "{\"file\":\"a.txt\"}", ctx, false);
    defer res1.deinit(std.testing.allocator);
    try std.testing.expectEqualStrings("echo: {\"file\":\"a.txt\"}", res1.output);

    // 2. Ask user -> onay yoksa UserConfirmationRequired
    try broker.capability_engine.addRule("ask_me", .ask_user, null);
    try broker.registerTool(.{
        .name = "ask_me",
        .description = "Onay ister",
        .schema_json = "{}",
    }, dummyEchoHandler);

    try std.testing.expectError(
        error.UserConfirmationRequired,
        broker.execute("ask_me", "{}", ctx, false),
    );

    // Onay verilirse çalışır
    var res2 = try broker.execute("ask_me", "{}", ctx, true);
    defer res2.deinit(std.testing.allocator);

    // 3. Deny -> ToolDenied
    try broker.capability_engine.addRule("secret_tool", .deny, null);
    try broker.registerTool(.{
        .name = "secret_tool",
        .description = "Yasaklı",
        .schema_json = "{}",
    }, dummyEchoHandler);

    try std.testing.expectError(
        error.ToolDenied,
        broker.execute("secret_tool", "{}", ctx, true),
    );

    // 4. Doom loop eşiği: read_file aynı argümanla 2. kez çağrılırsa DoomLoopDetected
    try std.testing.expectError(
        error.DoomLoopDetected,
        broker.execute("read_file", "{\"file\":\"a.txt\"}", ctx, false),
    );
}
