//! omnitrix-ledger: Filesystem Watcher Debounce ve Authoritative Rescan (tasarım Bölüm 6.1, 8, Kol C - C6).
//!
//! 1. Watcher olayları burst halinde debounce edilir (event burst debounce).
//! 2. Yalnızca watcher olayına dayanarak liste gösterilmez; settle olunca authoritative rescan yapılır.
//! 3. Dosya değişikliği olmadığı kesinleşmeden 'No changes' yazılmaz.
//! 4. Büyük/binary/conflict dosyalarda sahte 0/0 gösterilmez (Doğrulama 9).
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const mutation = @import("mutation.zig");
const hash_mod = @import("hash.zig");
const scan = @import("scan.zig");
const ledger_mod = @import("ledger.zig");

pub const Hash = mutation.Hash;
pub const MutationRecord = mutation.MutationRecord;
pub const MutationKind = mutation.MutationKind;
pub const Actor = mutation.Actor;
pub const Ledger = ledger_mod.Ledger;

pub const FsEventType = enum {
    created,
    modified,
    deleted,
    renamed,
};

pub const FsEvent = struct {
    path: []const u8,
    event_type: FsEventType,
    timestamp: i128,
};

pub const DebounceConfig = struct {
    /// Debounce bekleme penceresi (nanosaniye cinsinden, varsayılan 50ms).
    debounce_window_ns: i128 = 50 * 1_000_000,
    /// Bounded kuyruk sınırı.
    max_pending_events: usize = 1000,
};

pub const WatcherError = scan.ScanError || hash_mod.HashError || ledger_mod.LedgerError || error{
    DebounceNotSettled,
    QueueFull,
} || std.mem.Allocator.Error;

pub const RescanResult = struct {
    changed_count: usize,
    authoritative_verified: bool,
};

pub const WatcherDebounceQueue = struct {
    allocator: std.mem.Allocator,
    config: DebounceConfig,
    /// Path -> FsEventType (tekilleştirilmiş pending haritası).
    pending_events: std.StringHashMap(FsEventType),
    last_event_time: ?i128 = null,
    is_settled: bool = true,
    unverified_changes: bool = false,

    pub fn init(allocator: std.mem.Allocator, config: DebounceConfig) WatcherDebounceQueue {
        return .{
            .allocator = allocator,
            .config = config,
            .pending_events = std.StringHashMap(FsEventType).init(allocator),
            .last_event_time = null,
            .is_settled = true,
            .unverified_changes = false,
        };
    }

    pub fn deinit(self: *WatcherDebounceQueue) void {
        var it = self.pending_events.keyIterator();
        while (it.next()) |k| {
            self.allocator.free(k.*);
        }
        self.pending_events.deinit();
        self.* = undefined;
    }

    /// Watcher'dan gelen ham olayı kuyruğa ekler (burst debounce başlatır).
    pub fn pushEvent(self: *WatcherDebounceQueue, path: []const u8, event_type: FsEventType, now: i128) !void {
        if (self.pending_events.count() >= self.config.max_pending_events) {
            return error.QueueFull;
        }

        if (self.pending_events.get(path)) |_| {
            try self.pending_events.put(path, event_type);
        } else {
            const owned = try self.allocator.dupe(u8, path);
            errdefer self.allocator.free(owned);
            try self.pending_events.put(owned, event_type);
        }

        self.last_event_time = now;
        self.is_settled = false;
        self.unverified_changes = true;
    }

    /// Burst süresinin durulup durulmadığını kontrol eder.
    pub fn checkSettle(self: *WatcherDebounceQueue, now: i128) bool {
        const last = self.last_event_time orelse {
            self.is_settled = true;
            return true;
        };

        if (now - last >= self.config.debounce_window_ns) {
            self.is_settled = true;
            return true;
        }

        self.is_settled = false;
        return false;
    }

    /// Doğrulanmamış veya henüz durulmamış olay var mı?
    pub fn hasPendingUnverified(self: *const WatcherDebounceQueue) bool {
        return self.unverified_changes or self.pending_events.count() > 0 or !self.is_settled;
    }

    /// 'No changes' durumu ancak ve ancak tüm olaylar durulduğunda ve
    /// authoritative rescan ile 0 değişiklik doğrulandığında true döner.
    pub fn canDeclareNoChanges(self: *const WatcherDebounceQueue, ledger_entry_count: usize) bool {
        if (self.hasPendingUnverified()) return false;
        return ledger_entry_count == 0;
    }

    /// Settle gerçekleştikten sonra authoritative rescan çalıştırır ve ledger'ı günceller.
    pub fn authoritativeRescan(
        self: *WatcherDebounceQueue,
        dir: std.Io.Dir,
        io: std.Io,
        ledger: *Ledger,
        now: i128,
        max_hash_bytes: usize,
    ) WatcherError!RescanResult {
        if (!self.checkSettle(now)) {
            return error.DebounceNotSettled;
        }

        // Authoritative rescan: diskteki gerçek durumu tara
        const changed = try ledger.rescan(dir, io, self.allocator, max_hash_bytes);

        // Bekleyen ham watcher olaylarını temizle
        var it = self.pending_events.keyIterator();
        while (it.next()) |k| {
            self.allocator.free(k.*);
        }
        self.pending_events.clearRetainingCapacity();

        self.unverified_changes = false;
        self.last_event_time = null;

        return .{
            .changed_count = changed,
            .authoritative_verified = true,
        };
    }
};

test "watcher debounce: event burst durulmasi ve authoritative rescan (Dogrulama 9)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    var watcher = WatcherDebounceQueue.init(std.testing.allocator, .{
        .debounce_window_ns = 50 * 1_000_000, // 50ms
    });
    defer watcher.deinit();

    // Başlangıçta boş -> canDeclareNoChanges true
    try std.testing.expect(watcher.canDeclareNoChanges(ledger.entryCount()));

    // 1. Burst olayları gelir
    const t0: i128 = 100_000_000;
    try watcher.pushEvent("file1.txt", .created, t0);
    try watcher.pushEvent("file2.txt", .modified, t0 + 10_000_000); // 10ms sonra
    try watcher.pushEvent("file1.txt", .modified, t0 + 20_000_000); // 20ms sonra

    // Henüz 50ms dolmadı -> 'No changes' denemez
    try std.testing.expect(!watcher.canDeclareNoChanges(ledger.entryCount()));
    try std.testing.expect(watcher.hasPendingUnverified());

    // 30ms sonra kontrol -> henüz durulmadı
    try std.testing.expect(!watcher.checkSettle(t0 + 50_000_000));
    try std.testing.expectError(
        error.DebounceNotSettled,
        watcher.authoritativeRescan(tmp.dir, std.testing.io, &ledger, t0 + 50_000_000, 64 * 1024),
    );

    // Diske gerçek dosyaları yaz
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "file1.txt", .data = "content1" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "file2.txt", .data = "content2" });

    // 80ms sonra kontrol (t0 + 20ms + 60ms) -> duruldu
    const t_settled = t0 + 80_000_000;
    try std.testing.expect(watcher.checkSettle(t_settled));

    // Authoritative rescan çalışır
    const result = try watcher.authoritativeRescan(tmp.dir, std.testing.io, &ledger, t_settled, 64 * 1024);
    try std.testing.expect(result.authoritative_verified);
    try std.testing.expectEqual(@as(usize, 2), result.changed_count);
    try std.testing.expectEqual(@as(usize, 2), ledger.entryCount());
    try std.testing.expect(!watcher.hasPendingUnverified());
}

test "binary veya buyuk dosyalarda sahte 0/0 gosterilmez (Dogrulama 9)" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    // Küçük limit vererek dosyanın binary/too large olarak işaretlenmesini sağla
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "large.bin", .data = "0123456789ABCDEF" });

    const changed = try ledger.rescan(tmp.dir, std.testing.io, std.testing.allocator, 8); // limit 8 byte
    try std.testing.expectEqual(@as(usize, 1), changed);

    const rec = ledger.get("large.bin").?;
    try std.testing.expect(rec.kind == .binary);
    // Sahte 0/0 gösterilmemeli: additions ve deletions null olmalıdır
    try std.testing.expect(rec.additions == null);
    try std.testing.expect(rec.deletions == null);
}
