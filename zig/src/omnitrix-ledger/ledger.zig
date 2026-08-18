//! FileMutationLedger (tasarım Bölüm 6).
//!
//! Proje kökünün altındaki tüm gerçek dosya değişikliklerini izler. Her tarama
//! revision alır; MutationRecord'lar path başına tutulur. Attribution (6.2):
//! önceden kirli dosya sahiplenilmez, aynı dosyada farklı actor → mixed_actor.
//! Revert güvenliği (6.3): mevcut hash beklenen after_hash ile eşleşmezse işlem
//! non-destructive reddedilir (reset --hard / whole-worktree restore yok).

const std = @import("std");
const mutation = @import("mutation.zig");
const hash_mod = @import("hash.zig");
const scan = @import("scan.zig");

pub const MutationRecord = mutation.MutationRecord;
pub const MutationKind = mutation.MutationKind;
pub const Actor = mutation.Actor;
pub const Hash = mutation.Hash;

pub const LedgerError = scan.ScanError || error{
    InterveningChange, // araya giren user/external değişiklik — revert reddedilir
    FileMissing, // beklenen dosya yok
    ReadFailed,
    TooLarge,
};

/// Path başına en son bilinen durum.
const Entry = struct {
    /// Son gözlemlenen içerik hash'i (yoksa `null` = henüz hash'lenmedi/yok).
    last_hash: ?Hash,
    /// Bu path için en son kayıt (sahiplenilmiş).
    record: MutationRecord,
};

pub const Ledger = struct {
    allocator: std.mem.Allocator,
    /// Path → Entry. Path anahtarları record.path ile paylaşılır; ayrıca dupe
    /// edilmez (StringHashMap anahtarı record'a ait olan stringdir).
    entries: std.StringHashMap(*Entry),
    /// Sıradaki revision.
    revision: u64 = 1,

    pub fn init(allocator: std.mem.Allocator) Ledger {
        return .{ .allocator = allocator, .entries = std.StringHashMap(*Entry).init(allocator) };
    }

    pub fn deinit(self: *Ledger) void {
        var it = self.entries.valueIterator();
        while (it.next()) |entry_ptr| {
            entry_ptr.*.record.deinit(self.allocator);
            self.allocator.destroy(entry_ptr.*);
        }
        self.entries.deinit();
        self.* = undefined;
    }

    fn nextRevision(self: *Ledger) u64 {
        const r = self.revision;
        self.revision += 1;
        return r;
    }

    pub fn entryCount(self: *const Ledger) usize {
        return self.entries.count();
    }

    /// `path` için kayıt varsa döner (sahipliği yoktur).
    pub fn get(self: *Ledger, path: []const u8) ?*MutationRecord {
        const e = self.entries.get(path) orelse return null;
        return &e.record;
    }

    pub const RecordOptions = struct {
        agent_id: ?[]const u8 = null,
        session_id: ?[]const u8 = null,
        turn_id: ?u64 = null,
        operation_id: ?[]const u8 = null,
        additions: ?u64 = null,
        deletions: ?u64 = null,
        hunk_index: ?u64 = null,
        attribution_confidence: u8 = 100,
    };

    /// Yeni bir gözlem kaydeder veya mevcut kaydı günceller. Attribution kuralı
    /// (6.2): mevcut kayıt farklı actor'a aitse ve yeni gözlem de farklıysa
    /// kind `mixed_actor` olur — geri alma sessizce uygulanmaz.
    pub fn record(
        self: *Ledger,
        path: []const u8,
        kind: MutationKind,
        actor: Actor,
        observed_at: i128,
        after_hash: ?Hash,
        before_hash: ?Hash,
    ) LedgerError!*MutationRecord {
        return self.recordWithOptions(path, kind, actor, observed_at, after_hash, before_hash, .{});
    }

    /// Tüm attribution ve bağlam seçenekleriyle kayıt oluşturur / günceller.
    pub fn recordWithOptions(
        self: *Ledger,
        path: []const u8,
        kind: MutationKind,
        actor: Actor,
        observed_at: i128,
        after_hash: ?Hash,
        before_hash: ?Hash,
        options: RecordOptions,
    ) LedgerError!*MutationRecord {
        if (self.entries.get(path)) |existing| {
            // Önceden kirli dosya sahiplenilmez: actor değiştiyse mixed_actor (6.2).
            var final_kind = kind;
            if (existing.record.actor != actor and existing.record.actor != .external) {
                final_kind = .mixed_actor;
            }
            existing.record.revision = self.nextRevision();
            existing.record.kind = final_kind;
            existing.record.actor = actor;
            existing.record.observed_at = observed_at;
            existing.record.after_hash = after_hash;
            if (before_hash) |bh| existing.record.before_hash = bh;
            existing.last_hash = after_hash;
            existing.record.additions = options.additions;
            existing.record.deletions = options.deletions;
            existing.record.hunk_index = options.hunk_index;
            existing.record.attribution_confidence = options.attribution_confidence;
            try existing.record.setContext(
                self.allocator,
                options.operation_id,
                options.agent_id,
                options.session_id,
                options.turn_id,
            );
            return &existing.record;
        }

        var rec = try MutationRecord.init(self.allocator, self.nextRevision(), path, kind, actor, observed_at);
        errdefer rec.deinit(self.allocator);
        rec.after_hash = after_hash;
        rec.before_hash = before_hash;
        rec.additions = options.additions;
        rec.deletions = options.deletions;
        rec.hunk_index = options.hunk_index;
        rec.attribution_confidence = options.attribution_confidence;
        try rec.setContext(
            self.allocator,
            options.operation_id,
            options.agent_id,
            options.session_id,
            options.turn_id,
        );

        const entry = try self.allocator.create(Entry);
        entry.* = .{ .last_hash = after_hash, .record = rec };
        errdefer self.allocator.destroy(entry);

        try self.entries.put(entry.record.path, entry);
        return &entry.record;
    }

    /// Tüm projeyi yeniden tarar; hash'i değişen dosyalar `modified`, yeni
    /// dosyalar `added`, kaybolan dosyalar `deleted` olur. (Bölüm 9.1, 9.2)
    pub fn rescan(
        self: *Ledger,
        dir: std.Io.Dir,
        io: std.Io,
        allocator: std.mem.Allocator,
        max_hash_bytes: usize,
    ) LedgerError!usize {
        var scanned = try scan.scanProject(dir, io, allocator, .{});
        defer scanned.deinit();

        // Şu an diskte olan yollar.
        var seen = std.StringHashMap(void).init(allocator);
        defer seen.deinit();

        var changed: usize = 0;
        const now: i128 = @intCast(std.Io.Clock.now(.real, io).nanoseconds);

        for (scanned.files.items) |path| {
            try seen.put(path, {});
            const h = hash_mod.hashFile(dir, io, path, allocator, max_hash_bytes) catch |err| switch (err) {
                error.FileTooLarge => {
                    _ = try self.record(path, .binary, .external, now, null, null);
                    changed += 1;
                    continue;
                },
                else => return error.ReadFailed,
            };

            if (self.entries.get(path)) |existing| {
                const last = existing.last_hash orelse continue;
                if (!std.mem.eql(u8, &last, &h)) {
                    // Content hash değişti → modified (okuma/erişim değil, 6.2).
                    _ = try self.record(path, .modified, existing.record.actor, now, h, last);
                    changed += 1;
                }
            } else {
                _ = try self.record(path, .added, .external, now, h, null);
                changed += 1;
            }
        }

        // Diskte artık olmayan, ledger'da kayıtlı dosyalar → deleted.
        var to_delete = std.ArrayList([]const u8).empty;
        defer to_delete.deinit(self.allocator);
        var it = self.entries.keyIterator();
        while (it.next()) |path| {
            if (!seen.contains(path.*)) {
                try to_delete.append(self.allocator, path.*);
            }
        }
        for (to_delete.items) |path| {
            if (self.entries.get(path)) |existing| {
                _ = try self.record(path, .deleted, existing.record.actor, now, null, existing.last_hash);
                changed += 1;
            }
        }

        return changed;
    }

    /// Revert güvenliği (Bölüm 6.3): mevcut hash beklenen `expected_after_hash`
    /// ile eşleşmezse error.InterveningChange — hiçbir şey yazılmaz, non-destructive.
    pub fn verifyRevertPrecondition(
        self: *Ledger,
        dir: std.Io.Dir,
        io: std.Io,
        allocator: std.mem.Allocator,
        path: []const u8,
        expected_after_hash: Hash,
        max_hash_bytes: usize,
    ) LedgerError!void {
        _ = self;
        const current = hash_mod.hashFile(dir, io, path, allocator, max_hash_bytes) catch |err| switch (err) {
            error.FileTooLarge => return error.TooLarge,
            else => return error.ReadFailed,
        };
        if (!std.mem.eql(u8, &current, &expected_after_hash)) {
            // Araya giren user/external değişiklik — revert durdurulur (6.3 adım 3).
            return error.InterveningChange;
        }
    }
};

test "ledger kayit ve revision" {
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();
    const r1 = try ledger.record("a.txt", .added, .agent, 100, null, null);
    try std.testing.expectEqual(@as(u64, 1), r1.revision);
    const r2 = try ledger.record("a.txt", .modified, .agent, 200, null, null);
    try std.testing.expectEqual(@as(u64, 2), r2.revision);
    try std.testing.expectEqual(@as(usize, 1), ledger.entryCount());
}

test "attribution: farkli actor mixed_actor olur" {
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();
    _ = try ledger.record("shared.txt", .modified, .agent, 100, null, null);
    const rec = try ledger.record("shared.txt", .modified, .user, 200, null, null);
    try std.testing.expect(rec.kind == .mixed_actor);
}

test "rescan: gitignored dosya degisimi gorunur, untracked added olur" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = ".gitignore", .data = "secret.txt\n" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "secret.txt", .data = "v1" });

    const first = try ledger.rescan(tmp.dir, std.testing.io, std.testing.allocator, 64 * 1024);
    try std.testing.expectEqual(@as(usize, 2), first); // .gitignore + secret.txt added

    // Gitignored dosyayı agent değiştirir; panelde görünür (Doğrulama 1).
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "secret.txt", .data = "v2" });
    const second = try ledger.rescan(tmp.dir, std.testing.io, std.testing.allocator, 64 * 1024);
    try std.testing.expectEqual(@as(usize, 1), second); // secret.txt modified

    const rec = ledger.get("secret.txt").?;
    try std.testing.expect(rec.kind == .modified);
    try std.testing.expect(rec.actor == .external);
}

test "revert: eski hash ile non-destructive reddedilir (Dogrulama 10)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "f.txt", .data = "original" });
    const h_orig = hash_mod.hashBytes("original");

    // Ledger kaydı: f.txt → after_hash = h_orig
    _ = try ledger.record("f.txt", .added, .agent, 100, h_orig, null);

    // Araya user/external girer, içerik değişir (6.3 adım 3).
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "f.txt", .data = "tampered" });

    // Eski hash ile revert denenir → non-destructive red (Doğrulama 10).
    try std.testing.expectError(
        error.InterveningChange,
        ledger.verifyRevertPrecondition(tmp.dir, std.testing.io, std.testing.allocator, "f.txt", h_orig, 64 * 1024),
    );

    // Dosya olduğu gibi kalır — hiçbir şey yazılmadı.
    const content = try tmp.dir.readFileAlloc(std.testing.io, "f.txt", std.testing.allocator, .limited(1024));
    defer std.testing.allocator.free(content);
    try std.testing.expectEqualStrings("tampered", content);
}

test "revert: hash eslesirse on kosul gecer" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "ok.txt", .data = "stable" });
    const h = hash_mod.hashBytes("stable");
    _ = try ledger.record("ok.txt", .modified, .agent, 100, h, null);

    try ledger.verifyRevertPrecondition(tmp.dir, std.testing.io, std.testing.allocator, "ok.txt", h, 64 * 1024);
}
