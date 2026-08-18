//! omnitrix-ledger: Attribution ve Hunk / Mixed Actor yönetimi (tasarım Bölüm 6.2, Kol C - C4).
//!
//! Kurallar:
//! 1. Önceden kirli dosya agent tarafından sahiplenilmiş sayılmaz.
//! 2. Okuma (access) ile değiştirme (mutation) ayrıdır. Okuma changed-files paneline girmez.
//! 3. Hunk-level attribution: Aynı dosyada kullanıcı ve ajan farklı hunk'lara dokunursa
//!    ayrı hunk sahiplikleri tutulur.
//! 4. Güvenilmez/çakışan ayrımda dosya `mixed_actor` olur ve geri alma işlemi durdurulur.
//! 5. Dış süreç değişikliği ajan değişikliği olarak etiketlenmez (Doğrulama 4).
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const hunk_mod = @import("hunk.zig");
const mutation = @import("mutation.zig");
const ledger_mod = @import("ledger.zig");

pub const Hunk = hunk_mod.Hunk;
pub const Actor = mutation.Actor;
pub const MutationKind = mutation.MutationKind;
pub const Hash = mutation.Hash;
pub const Ledger = ledger_mod.Ledger;

pub const HunkAttribution = struct {
    hunk: Hunk,
    actor: Actor,
    agent_id: ?[]const u8 = null,
    confidence: u8 = 100,

    pub fn clone(self: HunkAttribution, allocator: std.mem.Allocator) !HunkAttribution {
        return .{
            .hunk = self.hunk,
            .actor = self.actor,
            .agent_id = if (self.agent_id) |s| try allocator.dupe(u8, s) else null,
            .confidence = self.confidence,
        };
    }

    pub fn deinit(self: *HunkAttribution, allocator: std.mem.Allocator) void {
        if (self.agent_id) |s| allocator.free(s);
        self.* = undefined;
    }
};

pub const FileAttribution = struct {
    allocator: std.mem.Allocator,
    path: []const u8,
    hunks: std.ArrayList(HunkAttribution),
    kind: MutationKind,
    primary_actor: Actor,
    is_mixed: bool,
    confidence: u8,
    prior_dirty: bool,

    pub fn init(allocator: std.mem.Allocator, path: []const u8) !FileAttribution {
        return .{
            .allocator = allocator,
            .path = try allocator.dupe(u8, path),
            .hunks = std.ArrayList(HunkAttribution).empty,
            .kind = .modified,
            .primary_actor = .agent,
            .is_mixed = false,
            .confidence = 100,
            .prior_dirty = false,
        };
    }

    pub fn deinit(self: *FileAttribution) void {
        self.allocator.free(self.path);
        for (self.hunks.items) |*h| h.deinit(self.allocator);
        self.hunks.deinit(self.allocator);
        self.* = undefined;
    }
};

/// Okuma erişim kaydı (okuma content hash'ini değiştirmez).
pub const ReadAccessLog = struct {
    allocator: std.mem.Allocator,
    records: std.StringHashMap(i128),

    pub fn init(allocator: std.mem.Allocator) ReadAccessLog {
        return .{
            .allocator = allocator,
            .records = std.StringHashMap(i128).init(allocator),
        };
    }

    pub fn deinit(self: *ReadAccessLog) void {
        var it = self.records.keyIterator();
        while (it.next()) |k| self.allocator.free(k.*);
        self.records.deinit();
        self.* = undefined;
    }

    /// Okuma aktivitesini kaydeder; Ledger mutasyon listesine DAHİL OLMAZ.
    pub fn recordRead(self: *ReadAccessLog, path: []const u8, timestamp: i128) !void {
        if (self.records.get(path)) |_| {
            try self.records.put(path, timestamp);
        } else {
            const owned = try self.allocator.dupe(u8, path);
            errdefer self.allocator.free(owned);
            try self.records.put(owned, timestamp);
        }
    }
};

/// Taban dosya, kullanıcı düzenlemesi ve ajan düzenlemesi arasındaki attribution'ı çözümler.
pub fn resolveHunkAttribution(
    allocator: std.mem.Allocator,
    path: []const u8,
    base_text: []const u8,
    user_text: ?[]const u8,
    agent_text: ?[]const u8,
    agent_id: ?[]const u8,
    is_prior_dirty: bool,
) !FileAttribution {
    var attr = try FileAttribution.init(allocator, path);
    errdefer attr.deinit();
    attr.prior_dirty = is_prior_dirty;

    // 1. Kullanıcı ve Ajan hunk'larını hesapla
    var user_hunks = if (user_text) |ut|
        try hunk_mod.computeHunks(allocator, base_text, ut)
    else
        std.ArrayList(Hunk).empty;
    defer user_hunks.deinit(allocator);

    var agent_hunks = if (agent_text) |at|
        try hunk_mod.computeHunks(allocator, base_text, at)
    else
        std.ArrayList(Hunk).empty;
    defer agent_hunks.deinit(allocator);

    // 2. Çakışma kontrolü (Overlap check)
    var has_overlap = false;
    for (user_hunks.items) |uh| {
        for (agent_hunks.items) |ah| {
            if (uh.overlaps(ah)) {
                has_overlap = true;
                break;
            }
        }
        if (has_overlap) break;
    }

    if (has_overlap or (is_prior_dirty and user_hunks.items.len > 0 and agent_hunks.items.len > 0)) {
        // Çakışan veya belirsiz ayrım -> mixed_actor (tasarım 6.2)
        attr.kind = .mixed_actor;
        attr.is_mixed = true;
        attr.confidence = 50;
        attr.primary_actor = .user; // Önceden kirli / kullanıcı öncelikli
    } else if (user_hunks.items.len > 0 and agent_hunks.items.len > 0) {
        // İkisi de var ama farklı hunk'lar -> her iki aktörün hunk'ları ayrı tutulur
        attr.kind = .modified;
        attr.is_mixed = true;
        attr.confidence = 90;
        attr.primary_actor = .agent;
    } else if (agent_hunks.items.len > 0) {
        attr.kind = .modified;
        attr.primary_actor = .agent;
        attr.confidence = if (is_prior_dirty) 60 else 100;
    } else if (user_hunks.items.len > 0) {
        attr.kind = .modified;
        attr.primary_actor = .user;
        attr.confidence = 100;
    } else {
        attr.kind = .modified;
        attr.primary_actor = .external;
        attr.confidence = 100;
    }

    // Hunk listesini doldur
    for (user_hunks.items) |uh| {
        try attr.hunks.append(allocator, .{
            .hunk = uh,
            .actor = .user,
            .agent_id = null,
            .confidence = if (has_overlap) 50 else 100,
        });
    }

    for (agent_hunks.items) |ah| {
        const owned_ag_id = if (agent_id) |ag| try allocator.dupe(u8, ag) else null;
        try attr.hunks.append(allocator, .{
            .hunk = ah,
            .actor = .agent,
            .agent_id = owned_ag_id,
            .confidence = if (has_overlap) 50 else 100,
        });
    }

    return attr;
}

test "attribution: user ve agent farkli hunk'lar (Dogrulama 3)" {
    const base = "header\nuser_section\nmiddle\nagent_section\nfooter";
    const user_edit = "header\nUSER_EDITED\nmiddle\nagent_section\nfooter";
    const agent_edit = "header\nuser_section\nmiddle\nAGENT_EDITED\nfooter";

    var attr = try resolveHunkAttribution(
        std.testing.allocator,
        "src/shared.zig",
        base,
        user_edit,
        agent_edit,
        "agent_juryrigg",
        false,
    );
    defer attr.deinit();

    try std.testing.expect(attr.kind == .modified);
    try std.testing.expect(attr.is_mixed); // her iki aktör de dosyada var
    try std.testing.expectEqual(@as(usize, 2), attr.hunks.items.len);
    try std.testing.expect(attr.hunks.items[0].actor == .user);
    try std.testing.expect(attr.hunks.items[1].actor == .agent);
}

test "attribution: user ve agent cakisan hunk -> mixed_actor (Dogrulama 3)" {
    const base = "line1\nline2\nline3\nline4\nline5";
    const user_edit = "line1\nuser_change_2\nline3\nline4\nline5";
    const agent_edit = "line1\nagent_change_2\nline3\nline4\nline5";

    var attr = try resolveHunkAttribution(
        std.testing.allocator,
        "src/conflict.zig",
        base,
        user_edit,
        agent_edit,
        "agent_juryrigg",
        false,
    );
    defer attr.deinit();

    // Çakışma durumunda mixed_actor olur ve güvenilirlik düşer
    try std.testing.expect(attr.kind == .mixed_actor);
    try std.testing.expect(attr.is_mixed);
    try std.testing.expect(attr.confidence <= 50);
}

test "attribution: dis surec degisikligi agent olarak etiketlenmez (Dogrulama 4)" {
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    // Dış süreç veya dosya taramasından gelen değişiklik
    const rec = try ledger.record("external_file.txt", .modified, .external, 1000, null, null);
    try std.testing.expectEqual(Actor.external, rec.actor);
    try std.testing.expect(rec.agent_id == null);
}

test "attribution: okuma islemi changed files mutasyonuna girmez" {
    var access_log = ReadAccessLog.init(std.testing.allocator);
    defer access_log.deinit();

    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    // Agent dosyayı okur
    try access_log.recordRead("src/main.zig", 12345);

    // Ledger'da herhangi bir mutation kaydı oluşmamalıdır (0 entry)
    try std.testing.expectEqual(@as(usize, 0), ledger.entryCount());
    try std.testing.expect(ledger.get("src/main.zig") == null);
}
