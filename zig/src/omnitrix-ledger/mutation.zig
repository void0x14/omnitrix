//! FileMutationLedger veri modeli (tasarım Bölüm 6.0).
//!
//! MutationRecord, proje kökü altındaki gerçek dosya değişikliğini izler:
//! revision, path, old_path?, before_hash?, after_hash?, kind, actor,
//! agent_id?, session_id?, turn_id?, operation_id?, additions?, deletions?,
//! hunk_index?, observed_at, attribution_confidence.

const std = @import("std");

/// Kayıtlı durumlar (tasarım Bölüm 5.2).
pub const MutationKind = enum {
    added,
    modified,
    deleted,
    renamed,
    copied,
    type_changed,
    binary,
    conflict,
    mixed_actor,
    stale,
    unavailable,

    pub fn label(self: MutationKind) []const u8 {
        return @tagName(self);
    }
};

/// Değişikliğin sahibi (tasarım Bölüm 6.1: kullanıcı veya dış süreç, aktif ajan
/// işlemi dışında görülen değişiklik).
pub const Actor = enum {
    agent,
    user,
    external,

    pub fn label(self: Actor) []const u8 {
        return @tagName(self);
    }
};

pub const Hash = [32]u8;

/// Tek dosya değişikliği kaydı. String alanlar sahiplenilir (allocator ile).
pub const MutationRecord = struct {
    revision: u64,
    path: []const u8,
    old_path: ?[]const u8 = null,
    before_hash: ?Hash = null,
    after_hash: ?Hash = null,
    kind: MutationKind,
    actor: Actor,
    agent_id: ?[]const u8 = null,
    session_id: ?[]const u8 = null,
    turn_id: ?u64 = null,
    operation_id: ?[]const u8 = null,
    additions: ?u64 = null,
    deletions: ?u64 = null,
    hunk_index: ?u64 = null,
    observed_at: i128,
    attribution_confidence: u8 = 100,

    /// `path` dahil tüm string alanları allocator ile kopyalar.
    pub fn init(
        allocator: std.mem.Allocator,
        revision: u64,
        path: []const u8,
        kind: MutationKind,
        actor: Actor,
        observed_at: i128,
    ) !MutationRecord {
        return .{
            .revision = revision,
            .path = try allocator.dupe(u8, path),
            .kind = kind,
            .actor = actor,
            .observed_at = observed_at,
        };
    }

    pub fn setContext(
        self: *MutationRecord,
        allocator: std.mem.Allocator,
        operation_id: ?[]const u8,
        agent_id: ?[]const u8,
        session_id: ?[]const u8,
        turn_id: ?u64,
    ) !void {
        if (self.operation_id) |s| allocator.free(s);
        self.operation_id = if (operation_id) |op| try allocator.dupe(u8, op) else null;

        if (self.agent_id) |s| allocator.free(s);
        self.agent_id = if (agent_id) |ag| try allocator.dupe(u8, ag) else null;

        if (self.session_id) |s| allocator.free(s);
        self.session_id = if (session_id) |sess| try allocator.dupe(u8, sess) else null;

        self.turn_id = turn_id;
    }

    pub fn deinit(self: *MutationRecord, allocator: std.mem.Allocator) void {
        allocator.free(self.path);
        if (self.old_path) |p| allocator.free(p);
        if (self.agent_id) |s| allocator.free(s);
        if (self.session_id) |s| allocator.free(s);
        if (self.operation_id) |s| allocator.free(s);
        self.* = undefined;
    }
};

test "mutation record init/deinit ve alanlar" {
    var rec = try MutationRecord.init(std.testing.allocator, 7, "src/main.zig", .modified, .agent, 123);
    defer rec.deinit(std.testing.allocator);
    try std.testing.expectEqualStrings("src/main.zig", rec.path);
    try std.testing.expectEqual(@as(u64, 7), rec.revision);
    try std.testing.expect(rec.kind == .modified);
    try std.testing.expect(rec.actor == .agent);
    try std.testing.expect(rec.before_hash == null);

    try rec.setContext(std.testing.allocator, "op_100", "agent_x", "sess_y", 42);
    try std.testing.expectEqualStrings("op_100", rec.operation_id.?);
    try std.testing.expectEqualStrings("agent_x", rec.agent_id.?);
    try std.testing.expectEqualStrings("sess_y", rec.session_id.?);
    try std.testing.expectEqual(@as(?u64, 42), rec.turn_id);
}

test "kind ve actor etiketleri" {
    try std.testing.expectEqualStrings("mixed_actor", @tagName(MutationKind.mixed_actor));
    try std.testing.expectEqualStrings("external", @tagName(Actor.external));
}
