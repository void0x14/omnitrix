//! omnitrix-permission: Tool capability kuralları ve yetkilendirme motoru (tasarım Bölüm 3.2, 7 Kol C - C1).
//!
//! Kurallar: allow, deny, ask_user.
//! Deny -> gizleme: Yetki kuralı bir tool'u 'deny' olarak değerlendirdiğinde,
//! o tool modele sunulan şema listesinden tamamen gizlenir (model yasak tool'u görmez).
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");

/// Yetkilendirme aksiyonu.
pub const CapabilityAction = enum {
    allow,
    deny,
    ask_user,

    pub fn label(self: CapabilityAction) []const u8 {
        return @tagName(self);
    }
};

/// Yetkilendirme kuralı.
pub const CapabilityRule = struct {
    pattern: []const u8,
    action: CapabilityAction,
    reason: ?[]const u8 = null,

    pub fn init(allocator: std.mem.Allocator, pattern: []const u8, action: CapabilityAction, reason: ?[]const u8) !CapabilityRule {
        return .{
            .pattern = try allocator.dupe(u8, pattern),
            .action = action,
            .reason = if (reason) |r| try allocator.dupe(u8, r) else null,
        };
    }

    pub fn deinit(self: *CapabilityRule, allocator: std.mem.Allocator) void {
        allocator.free(self.pattern);
        if (self.reason) |r| allocator.free(r);
        self.* = undefined;
    }
};

/// Kural değerlendirme sonucu.
pub const CapabilityDecision = struct {
    action: CapabilityAction,
    matched_pattern: ?[]const u8 = null,
    reason: ?[]const u8 = null,

    pub fn isAllowed(self: CapabilityDecision) bool {
        return self.action == .allow;
    }

    pub fn isDenied(self: CapabilityDecision) bool {
        return self.action == .deny;
    }

    pub fn requiresConfirmation(self: CapabilityDecision) bool {
        return self.action == .ask_user;
    }
};

/// Deseni eşleme yardımcısı (* wildcard, prefix*, *suffix, tam eşleşme).
pub fn matchesPattern(pattern: []const u8, name: []const u8) bool {
    if (std.mem.eql(u8, pattern, "*")) return true;
    if (std.mem.eql(u8, pattern, name)) return true;

    if (std.mem.endsWith(u8, pattern, "*")) {
        const prefix = pattern[0 .. pattern.len - 1];
        return std.mem.startsWith(u8, name, prefix);
    }

    if (std.mem.startsWith(u8, pattern, "*")) {
        const suffix = pattern[1..];
        return std.mem.endsWith(u8, name, suffix);
    }

    return false;
}

/// Yetkilendirme motoru.
pub const CapabilityEngine = struct {
    allocator: std.mem.Allocator,
    rules: std.ArrayList(CapabilityRule),
    default_action: CapabilityAction = .ask_user,

    pub fn init(allocator: std.mem.Allocator, default_action: CapabilityAction) CapabilityEngine {
        return .{
            .allocator = allocator,
            .rules = std.ArrayList(CapabilityRule).empty,
            .default_action = default_action,
        };
    }

    pub fn deinit(self: *CapabilityEngine) void {
        for (self.rules.items) |*r| {
            r.deinit(self.allocator);
        }
        self.rules.deinit(self.allocator);
        self.* = undefined;
    }

    /// Kural ekler (listenin sonuna eklenir; ilk eşleşen kural kazanır).
    pub fn addRule(self: *CapabilityEngine, pattern: []const u8, action: CapabilityAction, reason: ?[]const u8) !void {
        const rule = try CapabilityRule.init(self.allocator, pattern, action, reason);
        errdefer {
            var mut_rule = rule;
            mut_rule.deinit(self.allocator);
        }
        try self.rules.append(self.allocator, rule);
    }

    /// Tool adını kurallara göre değerlendirir.
    pub fn evaluate(self: *const CapabilityEngine, tool_name: []const u8) CapabilityDecision {
        for (self.rules.items) |rule| {
            if (matchesPattern(rule.pattern, tool_name)) {
                return .{
                    .action = rule.action,
                    .matched_pattern = rule.pattern,
                    .reason = rule.reason,
                };
            }
        }
        return .{
            .action = self.default_action,
            .matched_pattern = null,
            .reason = "default_policy",
        };
    }

    /// Deny -> gizleme: Model veya şema listesi için, denied olan tool'ların
    /// gizlenip gizlenmeyeceğini kontrol eder. Denied olan tool false döner.
    pub fn isToolVisible(self: *const CapabilityEngine, tool_name: []const u8) bool {
        const decision = self.evaluate(tool_name);
        // Yasaklanan tool'lar gizlenir (model görmez)
        return decision.action != .deny;
    }
};

test "pattern matching kurallari" {
    try std.testing.expect(matchesPattern("*", "anything"));
    try std.testing.expect(matchesPattern("read_*", "read_file"));
    try std.testing.expect(matchesPattern("read_*", "read_dir"));
    try std.testing.expect(!matchesPattern("read_*", "write_file"));
    try std.testing.expect(matchesPattern("*_danger", "rm_danger"));
    try std.testing.expect(matchesPattern("exact_tool", "exact_tool"));
    try std.testing.expect(!matchesPattern("exact_tool", "other_tool"));
}

test "capability engine allow deny ask_user ve gizleme" {
    var engine = CapabilityEngine.init(std.testing.allocator, .ask_user);
    defer engine.deinit();

    try engine.addRule("read_*", .allow, "safe read");
    try engine.addRule("secret_*", .deny, "restricted tool");
    try engine.addRule("delete_*", .ask_user, "destructive");

    // read_file -> allow (visible)
    const d1 = engine.evaluate("read_file");
    try std.testing.expectEqual(CapabilityAction.allow, d1.action);
    try std.testing.expect(engine.isToolVisible("read_file"));

    // secret_admin -> deny (HIDDEN / gizlenir)
    const d2 = engine.evaluate("secret_admin");
    try std.testing.expectEqual(CapabilityAction.deny, d2.action);
    try std.testing.expect(!engine.isToolVisible("secret_admin"));

    // delete_db -> ask_user (visible, requires confirmation)
    const d3 = engine.evaluate("delete_db");
    try std.testing.expectEqual(CapabilityAction.ask_user, d3.action);
    try std.testing.expect(engine.isToolVisible("delete_db"));

    // unknown_tool -> default ask_user (visible)
    const d4 = engine.evaluate("custom_tool");
    try std.testing.expectEqual(CapabilityAction.ask_user, d4.action);
    try std.testing.expect(engine.isToolVisible("custom_tool"));
}
