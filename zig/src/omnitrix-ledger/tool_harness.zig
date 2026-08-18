//! omnitrix-ledger: Tool mutation harness ve operasyon sahipliği (tasarım Bölüm 6.0, 6.1, Kol C - C2).
//!
//! Edit/write/create/delete tool'ları işlem öncesi (before_hash) ve sonrası (after_hash)
//! kesin hash'leri yakalar ve operation_id, agent_id, session_id, turn_id ile
//! doğrudan sahiplik (actor = .agent) kaydı oluşturur.
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const mutation = @import("mutation.zig");
const hash_mod = @import("hash.zig");
const ledger_mod = @import("ledger.zig");
const broker_mod = @import("../omnitrix-permission/broker.zig");

pub const Hash = mutation.Hash;
pub const MutationRecord = mutation.MutationRecord;
pub const MutationKind = mutation.MutationKind;
pub const Actor = mutation.Actor;
pub const Ledger = ledger_mod.Ledger;
pub const OperationContext = broker_mod.OperationContext;

pub const HarnessError = error{
    FileNotFound,
    PatternNotFound,
    WriteFailed,
    ReadFailed,
    DeleteFailed,
    FileTooLarge,
} || std.mem.Allocator.Error || ledger_mod.LedgerError;

/// Yardımcı: Üst klasör yolunu oluşturur.
fn ensureParentDir(dir: std.Io.Dir, io: std.Io, sub_path: []const u8) !void {
    if (std.fs.path.dirname(sub_path)) |parent| {
        if (parent.len > 0) {
            dir.createDirPath(io, parent) catch |err| switch (err) {
                error.PathAlreadyExists => {},
                else => return error.WriteFailed,
            };
        }
    }
}

/// Yeni dosya yazar veya mevcut dosyanın üzerine yazar. Kesin before/after hash yakalar.
pub fn writeFile(
    dir: std.Io.Dir,
    io: std.Io,
    allocator: std.mem.Allocator,
    sub_path: []const u8,
    data: []const u8,
    ctx: OperationContext,
    ledger: *Ledger,
    max_hash_bytes: usize,
) HarnessError!*MutationRecord {
    // 1. İşlem öncesi durum ve hash
    var before_h: ?Hash = null;
    var is_new = true;
    if (hash_mod.hashFile(dir, io, sub_path, allocator, max_hash_bytes)) |h| {
        before_h = h;
        is_new = false;
    } else |_| {
        is_new = true;
    }

    // 2. Üst klasörleri garanti et ve yaz
    try ensureParentDir(dir, io, sub_path);
    dir.writeFile(io, .{ .sub_path = sub_path, .data = data }) catch {
        return error.WriteFailed;
    };

    // 3. İşlem sonrası kesin hash
    const after_h = hash_mod.hashBytes(data);
    const kind: MutationKind = if (is_new) .added else .modified;
    const now: i128 = @intCast(std.Io.Clock.now(.real, io).nanoseconds);

    // 4. Ledger kaydı ve operasyon sahipliği
    return ledger.recordWithOptions(
        sub_path,
        kind,
        .agent,
        now,
        after_h,
        before_h,
        .{
            .agent_id = ctx.agent_id,
            .session_id = ctx.session_id,
            .turn_id = ctx.turn_id,
            .operation_id = ctx.operation_id,
            .attribution_confidence = 100,
        },
    );
}

/// Mevcut dosya üzerinde desen/metin değiştirme işlemi yapar.
pub fn editFile(
    dir: std.Io.Dir,
    io: std.Io,
    allocator: std.mem.Allocator,
    sub_path: []const u8,
    target_pattern: []const u8,
    replacement: []const u8,
    ctx: OperationContext,
    ledger: *Ledger,
    max_hash_bytes: usize,
) HarnessError!*MutationRecord {
    // 1. Mevcut içeriği oku
    const existing = dir.readFileAlloc(io, sub_path, allocator, .limited(max_hash_bytes)) catch |err| switch (err) {
        error.StreamTooLong => return error.FileTooLarge,
        else => return error.FileNotFound,
    };
    defer allocator.free(existing);

    const before_h = hash_mod.hashBytes(existing);

    // 2. Deseni ara
    const index = std.mem.indexOf(u8, existing, target_pattern) orelse return error.PatternNotFound;

    // 3. Yeni içeriği birleştir
    const prefix = existing[0..index];
    const suffix = existing[index + target_pattern.len ..];
    const new_content = try std.fmt.allocPrint(allocator, "{s}{s}{s}", .{ prefix, replacement, suffix });
    defer allocator.free(new_content);

    // 4. Dosyaya yaz
    dir.writeFile(io, .{ .sub_path = sub_path, .data = new_content }) catch {
        return error.WriteFailed;
    };

    // 5. İşlem sonrası hash ve ledger kaydı
    const after_h = hash_mod.hashBytes(new_content);
    const now: i128 = @intCast(std.Io.Clock.now(.real, io).nanoseconds);

    return ledger.recordWithOptions(
        sub_path,
        .modified,
        .agent,
        now,
        after_h,
        before_h,
        .{
            .agent_id = ctx.agent_id,
            .session_id = ctx.session_id,
            .turn_id = ctx.turn_id,
            .operation_id = ctx.operation_id,
            .attribution_confidence = 100,
        },
    );
}

/// Dosyayı siler ve ledger'a deleted kaydı düşer.
pub fn deleteFile(
    dir: std.Io.Dir,
    io: std.Io,
    allocator: std.mem.Allocator,
    sub_path: []const u8,
    ctx: OperationContext,
    ledger: *Ledger,
    max_hash_bytes: usize,
) HarnessError!*MutationRecord {
    // 1. Önceki hash'i al
    const before_h = hash_mod.hashFile(dir, io, sub_path, allocator, max_hash_bytes) catch {
        return error.FileNotFound;
    };

    // 2. Sil
    dir.deleteFile(io, sub_path) catch {
        return error.DeleteFailed;
    };

    const now: i128 = @intCast(std.Io.Clock.now(.real, io).nanoseconds);

    // 3. Ledger kaydı (after_hash = null)
    return ledger.recordWithOptions(
        sub_path,
        .deleted,
        .agent,
        now,
        null,
        before_h,
        .{
            .agent_id = ctx.agent_id,
            .session_id = ctx.session_id,
            .turn_id = ctx.turn_id,
            .operation_id = ctx.operation_id,
            .attribution_confidence = 100,
        },
    );
}

test "tool harness: untracked dosya olusturma ve sahiplik (Dogrulama 2)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const ctx = OperationContext{
        .operation_id = "op_create_1",
        .agent_id = "agent_juryrigg",
        .session_id = "sess_main",
        .turn_id = 10,
    };

    const rec = try writeFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        "src/new_mod.zig",
        "pub const x = 42;",
        ctx,
        &ledger,
        64 * 1024,
    );

    try std.testing.expectEqualStrings("src/new_mod.zig", rec.path);
    try std.testing.expect(rec.kind == .added);
    try std.testing.expect(rec.actor == .agent);
    try std.testing.expect(rec.before_hash == null);
    try std.testing.expect(rec.after_hash != null);
    try std.testing.expectEqualStrings("op_create_1", rec.operation_id.?);
    try std.testing.expectEqualStrings("agent_juryrigg", rec.agent_id.?);
    try std.testing.expectEqual(@as(?u64, 10), rec.turn_id);
}

test "tool harness: gitignored dosya yazma ve sahiplik (Dogrulama 1)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = ".gitignore", .data = "debug.log\n" });

    const ctx = OperationContext{
        .operation_id = "op_log_write",
        .agent_id = "agent_executor",
        .session_id = "sess_main",
        .turn_id = 1,
    };

    const rec = try writeFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        "debug.log",
        "log line 1\n",
        ctx,
        &ledger,
        64 * 1024,
    );

    try std.testing.expectEqualStrings("debug.log", rec.path);
    try std.testing.expect(rec.kind == .added);
    try std.testing.expect(rec.actor == .agent);
    try std.testing.expectEqualStrings("agent_executor", rec.agent_id.?);
}

test "tool harness: editFile ve deleteFile ile kesin hash yakalama" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "config.json", .data = "{\"debug\": false}" });
    const h_initial = hash_mod.hashBytes("{\"debug\": false}");

    const ctx = OperationContext{
        .operation_id = "op_edit_cfg",
        .agent_id = "agent_fixer",
        .session_id = "sess_cfg",
        .turn_id = 2,
    };

    // Edit
    const edit_rec = try editFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        "config.json",
        "false",
        "true",
        ctx,
        &ledger,
        64 * 1024,
    );

    try std.testing.expect(edit_rec.kind == .modified);
    try std.testing.expectEqualSlices(u8, &h_initial, &edit_rec.before_hash.?);
    try std.testing.expectEqualSlices(u8, &hash_mod.hashBytes("{\"debug\": true}"), &edit_rec.after_hash.?);

    // Delete
    const del_rec = try deleteFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        "config.json",
        ctx,
        &ledger,
        64 * 1024,
    );

    try std.testing.expect(del_rec.kind == .deleted);
    try std.testing.expect(del_rec.after_hash == null);
    try std.testing.expectEqualSlices(u8, &hash_mod.hashBytes("{\"debug\": true}"), &del_rec.before_hash.?);
}
