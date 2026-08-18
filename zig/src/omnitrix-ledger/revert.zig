//! omnitrix-ledger: 7-Adımlı Güvenli Revert Motoru (tasarım Bölüm 6.3, Kol C - C5).
//!
//! Revert Akışı:
//! 1. Scope kesinleştirme (Scope validation)
//! 2. Mevcut hash ile beklenen after_hash karşılaştırması
//! 3. Araya giren değişiklik varsa durdurma (InterveningChange)
//! 4. Recovery backup oluşturma
//! 5. Yalnızca seçilen dosya/hunk değişikliğini uygulama
//! 6. Post-revert hash doğrulaması (başarısızsa backup'tan kurtarma)
//! 7. Ledger ve durum güncellemesi
//!
//! Kesin Kural: Geniş kapsamlı `reset --hard` veya worktree restore YOKTUR.
//! Hata durumunda işlem tamamen non-destructive'dir (Doğrulama 10).
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const hash_mod = @import("hash.zig");
const mutation = @import("mutation.zig");
const ledger_mod = @import("ledger.zig");

pub const Hash = mutation.Hash;
pub const MutationRecord = mutation.MutationRecord;
pub const MutationKind = mutation.MutationKind;
pub const Actor = mutation.Actor;
pub const Ledger = ledger_mod.Ledger;

pub const RevertError = error{
    ScopeNotFound,
    MixedActorRevertRefused,
    InterveningChange,
    RecoveryBackupFailed,
    RevertApplyFailed,
    RevertVerificationFailed,
    PreImageRequired,
    ReadFailed,
    WriteFailed,
    DeleteFailed,
    FileTooLarge,
} || std.mem.Allocator.Error || ledger_mod.LedgerError;

pub const RevertOptions = struct {
    force_mixed_actor: bool = false,
    max_hash_bytes: usize = 64 * 1024 * 1024,
};

pub const RevertResult = struct {
    path: []const u8,
    reverted_to_hash: ?Hash,
    was_deleted: bool,
};

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

/// 7-Adımlı Güvenli Revert Fonksiyonu.
pub fn safeRevertFile(
    dir: std.Io.Dir,
    io: std.Io,
    allocator: std.mem.Allocator,
    ledger: *Ledger,
    path: []const u8,
    pre_image_content: ?[]const u8,
    options: RevertOptions,
) RevertError!RevertResult {
    // Adım 1: Scope kesinleştirme
    const rec = ledger.get(path) orelse return error.ScopeNotFound;
    const target_kind = rec.kind;
    const target_before_hash = rec.before_hash;
    const target_after_hash = rec.after_hash;

    // Mixed actor kontrolü: Belirsiz/çakışan aktör değişikliği onay olmadan reddedilir
    if (target_kind == .mixed_actor and !options.force_mixed_actor) {
        return error.MixedActorRevertRefused;
    }

    // Adım 2 & 3: Mevcut hash kontrolü ve araya giren değişiklik kontrolü
    if (target_after_hash) |expected_after| {
        const current_h = hash_mod.hashFile(dir, io, path, allocator, options.max_hash_bytes) catch {
            return error.InterveningChange;
        };

        if (!std.mem.eql(u8, &current_h, &expected_after)) {
            // Araya giren kullanıcı veya dış süreç değişikliği -> non-destructive durdur (Doğrulama 10)
            return error.InterveningChange;
        }
    } else {
        // after_hash null ise dosya diskte olmamalıdır
        if (hash_mod.hashFile(dir, io, path, allocator, options.max_hash_bytes)) |_| {
            return error.InterveningChange;
        } else |_| {}
    }

    // Adım 4: Recovery backup oluştur
    var recovery_backup: ?[]u8 = null;
    defer {
        if (recovery_backup) |b| allocator.free(b);
    }

    if (target_after_hash != null) {
        const current_data = dir.readFileAlloc(io, path, allocator, .limited(options.max_hash_bytes)) catch {
            return error.RecoveryBackupFailed;
        };
        recovery_backup = current_data;
    }

    // Adım 5: Yalnızca seçilen dosya için revert uygula
    var was_deleted = false;
    if (target_kind == .added or target_before_hash == null) {
        // Yeni oluşturulmuş dosya geri alınınca silinir
        dir.deleteFile(io, path) catch {
            return error.DeleteFailed;
        };
        was_deleted = true;
    } else {
        // Modified veya deleted dosya için pre_image içeriği yazılır
        const pre_content = pre_image_content orelse return error.PreImageRequired;
        try ensureParentDir(dir, io, path);
        dir.writeFile(io, .{ .sub_path = path, .data = pre_content }) catch {
            return error.WriteFailed;
        };
    }

    // Adım 6: Post-revert hash doğrulaması
    if (!was_deleted) {
        const actual_pre_h = hash_mod.hashFile(dir, io, path, allocator, options.max_hash_bytes) catch {
            // Geri yükle ve hata dön
            if (recovery_backup) |backup| {
                _ = dir.writeFile(io, .{ .sub_path = path, .data = backup }) catch {};
            }
            return error.RevertVerificationFailed;
        };

        if (target_before_hash) |expected_before| {
            if (!std.mem.eql(u8, &actual_pre_h, &expected_before)) {
                // Hash eşleşmedi! Backup'tan geri al ve başarısız ol
                if (recovery_backup) |backup| {
                    _ = dir.writeFile(io, .{ .sub_path = path, .data = backup }) catch {};
                }
                return error.RevertVerificationFailed;
            }
        }
    }

    // Adım 7: Ledger ve durum güncellemesi
    const now: i128 = @intCast(std.Io.Clock.now(.real, io).nanoseconds);
    if (was_deleted) {
        _ = try ledger.recordWithOptions(
            path,
            .deleted,
            .agent,
            now,
            null,
            target_after_hash,
            .{
                .attribution_confidence = 100,
            },
        );
    } else {
        _ = try ledger.recordWithOptions(
            path,
            .modified,
            .agent,
            now,
            target_before_hash,
            target_after_hash,
            .{
                .attribution_confidence = 100,
            },
        );
    }

    return .{
        .path = path,
        .reverted_to_hash = target_before_hash,
        .was_deleted = was_deleted,
    };
}

test "7-step revert: basarili modified dosya geri alma" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const original_text = "fn main() { original(); }";
    const modified_text = "fn main() { modified(); }";
    const h_orig = hash_mod.hashBytes(original_text);
    const h_mod = hash_mod.hashBytes(modified_text);

    // Diske modified halini yaz
    try tmp.dir.createDirPath(std.testing.io, "src");
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "src/app.zig", .data = modified_text });

    // Ledger kaydı
    _ = try ledger.recordWithOptions(
        "src/app.zig",
        .modified,
        .agent,
        100,
        h_mod,
        h_orig,
        .{},
    );

    // Revert uygula
    const res = try safeRevertFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        &ledger,
        "src/app.zig",
        original_text,
        .{},
    );

    try std.testing.expectEqualStrings("src/app.zig", res.path);
    try std.testing.expect(!res.was_deleted);
    try std.testing.expectEqualSlices(u8, &h_orig, &res.reverted_to_hash.?);

    // Diskteki dosyanın orijinal haline döndüğünü doğrula
    const reverted_content = try tmp.dir.readFileAlloc(std.testing.io, "src/app.zig", std.testing.allocator, .limited(1024));
    defer std.testing.allocator.free(reverted_content);
    try std.testing.expectEqualStrings(original_text, reverted_content);
}

test "7-step revert: basarili added dosya silme" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const added_text = "temporary file content";
    const h_add = hash_mod.hashBytes(added_text);

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "temp.txt", .data = added_text });

    _ = try ledger.recordWithOptions(
        "temp.txt",
        .added,
        .agent,
        100,
        h_add,
        null,
        .{},
    );

    const res = try safeRevertFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        &ledger,
        "temp.txt",
        null,
        .{},
    );

    try std.testing.expect(res.was_deleted);

    // Diskte artık dosya olmamalıdır
    try std.testing.expectError(
        error.FileNotFound,
        tmp.dir.openFile(std.testing.io, "temp.txt", .{}),
    );
}

test "7-step revert: araya giren degisiklikte non-destructive red (Dogrulama 10)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const agent_text = "agent output";
    const user_tampered = "user tampered this simultaneously";
    const h_agent = hash_mod.hashBytes(agent_text);
    const h_orig = hash_mod.hashBytes("original text");

    // Diske user tarafından değiştirilmiş içerik yazılır
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "conflict.zig", .data = user_tampered });

    // Ledger ajan değişikliğini bekliyor
    _ = try ledger.recordWithOptions(
        "conflict.zig",
        .modified,
        .agent,
        100,
        h_agent,
        h_orig,
        .{},
    );

    // Revert denenir -> InterveningChange ile reddedilir
    try std.testing.expectError(
        error.InterveningChange,
        safeRevertFile(
            tmp.dir,
            std.testing.io,
            std.testing.allocator,
            &ledger,
            "conflict.zig",
            "original text",
            .{},
        ),
    );

    // Dosya içeriği korunur (non-destructive)
    const content = try tmp.dir.readFileAlloc(std.testing.io, "conflict.zig", std.testing.allocator, .limited(1024));
    defer std.testing.allocator.free(content);
    try std.testing.expectEqualStrings(user_tampered, content);
}

test "7-step revert: mixed_actor durumunda onaysiz revert reddi" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const text = "mixed content";
    const h = hash_mod.hashBytes(text);
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "mixed.zig", .data = text });

    _ = try ledger.recordWithOptions(
        "mixed.zig",
        .mixed_actor,
        .agent,
        100,
        h,
        null,
        .{},
    );

    // Onaysız revert reddedilir
    try std.testing.expectError(
        error.MixedActorRevertRefused,
        safeRevertFile(
            tmp.dir,
            std.testing.io,
            std.testing.allocator,
            &ledger,
            "mixed.zig",
            null,
            .{ .force_mixed_actor = false },
        ),
    );
}
