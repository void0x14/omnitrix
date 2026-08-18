//! omnitrix-ledger: Shell komutu değişikliği ve bounded pre/post snapshot (tasarım Bölüm 6.1, Kol C - C3).
//!
//! Shell komutları dosya sistemini kontrolsüz değiştirebileceğinden, işlem sınırında
//! bounded pre-snapshot ve post-snapshot alınır.
//! Değişen/eklenen/silinen dosyalar tespit edilir ve `operation_id` ile ledger'a
//! doğrudan ajan sahipliği (`actor = .agent`) olarak işlenir (Doğrulama 5).
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const scan = @import("scan.zig");
const hash_mod = @import("hash.zig");
const mutation = @import("mutation.zig");
const ledger_mod = @import("ledger.zig");
const broker_mod = @import("../omnitrix-permission/broker.zig");

pub const Hash = mutation.Hash;
pub const MutationRecord = mutation.MutationRecord;
pub const MutationKind = mutation.MutationKind;
pub const Actor = mutation.Actor;
pub const Ledger = ledger_mod.Ledger;
pub const OperationContext = broker_mod.OperationContext;

pub const SnapshotError = scan.ScanError || hash_mod.HashError || ledger_mod.LedgerError || std.mem.Allocator.Error || error{
    SnapshotFailed,
};

/// Dosya yolu -> SHA-256 hash haritası.
pub const SnapshotMap = struct {
    allocator: std.mem.Allocator,
    entries: std.StringHashMap(Hash),

    pub fn init(allocator: std.mem.Allocator) SnapshotMap {
        return .{
            .allocator = allocator,
            .entries = std.StringHashMap(Hash).init(allocator),
        };
    }

    pub fn deinit(self: *SnapshotMap) void {
        var it = self.entries.keyIterator();
        while (it.next()) |k| {
            self.allocator.free(k.*);
        }
        self.entries.deinit();
        self.* = undefined;
    }

    pub fn put(self: *SnapshotMap, path: []const u8, h: Hash) !void {
        const owned = try self.allocator.dupe(u8, path);
        errdefer self.allocator.free(owned);
        try self.entries.put(owned, h);
    }

    pub fn get(self: *const SnapshotMap, path: []const u8) ?Hash {
        return self.entries.get(path);
    }
};

/// Projenin anlık bounded hash snapshot'ını alır.
pub fn takeSnapshot(
    dir: std.Io.Dir,
    io: std.Io,
    allocator: std.mem.Allocator,
    options: scan.ScanOptions,
    max_hash_bytes: usize,
) SnapshotError!SnapshotMap {
    var scanned = try scan.scanProject(dir, io, allocator, options);
    defer scanned.deinit();

    var snap = SnapshotMap.init(allocator);
    errdefer snap.deinit();

    for (scanned.files.items) |path| {
        const h = hash_mod.hashFile(dir, io, path, allocator, max_hash_bytes) catch |err| switch (err) {
            error.FileTooLarge => {
                // Büyük dosya için boş/özel hash
                var dummy_h: Hash = undefined;
                @memset(&dummy_h, 0xFF);
                try snap.put(path, dummy_h);
                continue;
            },
            else => return error.SnapshotFailed,
        };
        try snap.put(path, h);
    }

    return snap;
}

pub const ModifiedEntry = struct {
    path: []const u8,
    before_hash: Hash,
    after_hash: Hash,
};

pub const AddedEntry = struct {
    path: []const u8,
    after_hash: Hash,
};

pub const DeletedEntry = struct {
    path: []const u8,
    before_hash: Hash,
};

pub const SnapshotDiff = struct {
    allocator: std.mem.Allocator,
    added: std.ArrayList(AddedEntry),
    modified: std.ArrayList(ModifiedEntry),
    deleted: std.ArrayList(DeletedEntry),

    pub fn init(allocator: std.mem.Allocator) SnapshotDiff {
        return .{
            .allocator = allocator,
            .added = std.ArrayList(AddedEntry).empty,
            .modified = std.ArrayList(ModifiedEntry).empty,
            .deleted = std.ArrayList(DeletedEntry).empty,
        };
    }

    pub fn deinit(self: *SnapshotDiff) void {
        for (self.added.items) |e| self.allocator.free(e.path);
        for (self.modified.items) |e| self.allocator.free(e.path);
        for (self.deleted.items) |e| self.allocator.free(e.path);
        self.added.deinit(self.allocator);
        self.modified.deinit(self.allocator);
        self.deleted.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn totalChanges(self: *const SnapshotDiff) usize {
        return self.added.items.len + self.modified.items.len + self.deleted.items.len;
    }
};

/// İki snapshot arasındaki farkı çıkarır.
pub fn diffSnapshots(
    allocator: std.mem.Allocator,
    pre: *const SnapshotMap,
    post: *const SnapshotMap,
) !SnapshotDiff {
    var diff = SnapshotDiff.init(allocator);
    errdefer diff.deinit();

    // 1. Post'taki dosyalar (added veya modified kontrolü)
    var post_it = post.entries.iterator();
    while (post_it.next()) |entry| {
        const path = entry.key_ptr.*;
        const post_h = entry.value_ptr.*;

        if (pre.get(path)) |pre_h| {
            if (!std.mem.eql(u8, &pre_h, &post_h)) {
                const owned_path = try allocator.dupe(u8, path);
                try diff.modified.append(allocator, .{
                    .path = owned_path,
                    .before_hash = pre_h,
                    .after_hash = post_h,
                });
            }
        } else {
            const owned_path = try allocator.dupe(u8, path);
            try diff.added.append(allocator, .{
                .path = owned_path,
                .after_hash = post_h,
            });
        }
    }

    // 2. Pre'de olup Post'ta olmayanlar (deleted)
    var pre_it = pre.entries.iterator();
    while (pre_it.next()) |entry| {
        const path = entry.key_ptr.*;
        const pre_h = entry.value_ptr.*;

        if (post.get(path) == null) {
            const owned_path = try allocator.dupe(u8, path);
            try diff.deleted.append(allocator, .{
                .path = owned_path,
                .before_hash = pre_h,
            });
        }
    }

    return diff;
}

/// Diff sonuçlarını Ledger'a ajan sahipliği (`actor = .agent`) ve operation bağlamıyla kaydeder.
pub fn applyDiffToLedger(
    diff: *const SnapshotDiff,
    ctx: OperationContext,
    ledger: *Ledger,
    now: i128,
) !usize {
    var recorded: usize = 0;

    for (diff.added.items) |item| {
        _ = try ledger.recordWithOptions(
            item.path,
            .added,
            .agent,
            now,
            item.after_hash,
            null,
            .{
                .operation_id = ctx.operation_id,
                .agent_id = ctx.agent_id,
                .session_id = ctx.session_id,
                .turn_id = ctx.turn_id,
                .attribution_confidence = 100,
            },
        );
        recorded += 1;
    }

    for (diff.modified.items) |item| {
        _ = try ledger.recordWithOptions(
            item.path,
            .modified,
            .agent,
            now,
            item.after_hash,
            item.before_hash,
            .{
                .operation_id = ctx.operation_id,
                .agent_id = ctx.agent_id,
                .session_id = ctx.session_id,
                .turn_id = ctx.turn_id,
                .attribution_confidence = 100,
            },
        );
        recorded += 1;
    }

    for (diff.deleted.items) |item| {
        _ = try ledger.recordWithOptions(
            item.path,
            .deleted,
            .agent,
            now,
            null,
            item.before_hash,
            .{
                .operation_id = ctx.operation_id,
                .agent_id = ctx.agent_id,
                .session_id = ctx.session_id,
                .turn_id = ctx.turn_id,
                .attribution_confidence = 100,
            },
        );
        recorded += 1;
    }

    return recorded;
}

test "shell snapshot: oncesi/sonrasi farki ve ledger sahipligi (Dogrulama 5)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "build.zig", .data = "const std = @import(\"std\");" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "temp.txt", .data = "temp file" });

    // 1. Pre-snapshot al
    var pre_snap = try takeSnapshot(tmp.dir, std.testing.io, std.testing.allocator, .{}, 64 * 1024);
    defer pre_snap.deinit();

    // 2. Shell komutunun çalıştığını simüle et (yeni dosya oluştur, birini değiştir, birini sil)
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "build.zig", .data = "const std = @import(\"std\");\n// modified" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "generated.rs", .data = "fn main() {}" });
    try tmp.dir.deleteFile(std.testing.io, "temp.txt");

    // 3. Post-snapshot al
    var post_snap = try takeSnapshot(tmp.dir, std.testing.io, std.testing.allocator, .{}, 64 * 1024);
    defer post_snap.deinit();

    // 4. Fark çıkar
    var diff = try diffSnapshots(std.testing.allocator, &pre_snap, &post_snap);
    defer diff.deinit();

    try std.testing.expectEqual(@as(usize, 1), diff.added.items.len);
    try std.testing.expectEqualStrings("generated.rs", diff.added.items[0].path);

    try std.testing.expectEqual(@as(usize, 1), diff.modified.items.len);
    try std.testing.expectEqualStrings("build.zig", diff.modified.items[0].path);

    try std.testing.expectEqual(@as(usize, 1), diff.deleted.items.len);
    try std.testing.expectEqualStrings("temp.txt", diff.deleted.items[0].path);

    // 5. Ledger'a uygula
    const ctx = OperationContext{
        .operation_id = "op_shell_exec_42",
        .agent_id = "agent_executor",
        .session_id = "sess_shell",
        .turn_id = 5,
    };

    const count = try applyDiffToLedger(&diff, ctx, &ledger, 123456);
    try std.testing.expectEqual(@as(usize, 3), count);

    // Doğrulama 5 kontrolü: shell komutu sonrası ledger kaydı gelir ve ajan sahiplidir
    const gen_rec = ledger.get("generated.rs").?;
    try std.testing.expect(gen_rec.kind == .added);
    try std.testing.expect(gen_rec.actor == .agent);
    try std.testing.expectEqualStrings("op_shell_exec_42", gen_rec.operation_id.?);

    const bld_rec = ledger.get("build.zig").?;
    try std.testing.expect(bld_rec.kind == .modified);
    try std.testing.expect(bld_rec.actor == .agent);
    try std.testing.expectEqualStrings("op_shell_exec_42", bld_rec.operation_id.?);

    const del_rec = ledger.get("temp.txt").?;
    try std.testing.expect(del_rec.kind == .deleted);
    try std.testing.expect(del_rec.actor == .agent);
}
