//! omnitrix-runtime/metrics (tasarım Bölüm 8, Faz 9 — F9.1).
//!
//! Uzun koşu ve 24/7 soak metrik ölçüm altyapısı:
//! - OS RSS (Resident Set Size) takibi (/proc/self/statm veya getrusage)
//! - TrackedAllocator: Detaylı bellek tahsis/serbest bırakma, zirve bellek ve sıfır sızıntı takibi
//! - Event Lag Tracker: Olay kuyruklama-işleme gecikmesi (dispatch latency, min/max/avg/p99)
//! - First-Frame Tracker: İlk kare render gecikmesi ölçümü
//! - Shutdown Tracker: Deterministik kapanış süresi ölçümü
//! - RuntimeMetricsCollector: Birleşik metrik toplayıcı ve özet raporlayıcı
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR (I6 disiplini).

const std = @import("std");
const builtin = @import("builtin");
const io = @import("../omnitrix-io/io.zig");

/// Bellek kullanım ve tahsis metrikleri.
pub const MemoryMetrics = struct {
    alloc_count: u64 = 0,
    free_count: u64 = 0,
    realloc_count: u64 = 0,
    current_allocated_bytes: usize = 0,
    peak_allocated_bytes: usize = 0,
    total_allocated_bytes: u64 = 0,
    total_freed_bytes: u64 = 0,
    active_allocations: usize = 0,

    /// Bellek sızıntısı var mı kontrol eder (aktif tahsis veya bayt > 0).
    pub fn hasLeaks(self: MemoryMetrics) bool {
        return self.active_allocations > 0 or self.current_allocated_bytes > 0;
    }

    /// Bellek büyümesinin belirli bir tolerans içinde düz (flat) kalıp kalmadığını kontrol eder.
    pub fn isFlat(self: MemoryMetrics, baseline_bytes: usize, tolerance_bytes: usize) bool {
        if (self.current_allocated_bytes > baseline_bytes) {
            return (self.current_allocated_bytes - baseline_bytes) <= tolerance_bytes;
        } else {
            return (baseline_bytes - self.current_allocated_bytes) <= tolerance_bytes;
        }
    }
};

/// Herhangi bir Allocator'ı saran ve her tahsis/serbest bırakma işlemini kesin takip eden wrapper.
pub const TrackedAllocator = struct {
    parent_allocator: std.mem.Allocator,
    metrics: MemoryMetrics = .{},

    pub fn init(parent: std.mem.Allocator) TrackedAllocator {
        return .{
            .parent_allocator = parent,
            .metrics = .{},
        };
    }

    pub fn allocator(self: *TrackedAllocator) std.mem.Allocator {
        return .{
            .ptr = self,
            .vtable = &.{
                .alloc = allocFn,
                .resize = resizeFn,
                .remap = remapFn,
                .free = freeFn,
            },
        };
    }

    pub fn getMetrics(self: *const TrackedAllocator) MemoryMetrics {
        return self.metrics;
    }

    pub fn resetMetrics(self: *TrackedAllocator) void {
        self.metrics = .{
            .current_allocated_bytes = self.metrics.current_allocated_bytes,
            .peak_allocated_bytes = self.metrics.current_allocated_bytes,
            .active_allocations = self.metrics.active_allocations,
        };
    }

    pub fn assertNoLeaks(self: *const TrackedAllocator) !void {
        if (self.metrics.hasLeaks()) {
            return error.MemoryLeakDetected;
        }
    }

    fn allocFn(ctx: *anyopaque, len: usize, ptr_align: std.mem.Alignment, ret_addr: usize) ?[*]u8 {
        const self: *TrackedAllocator = @ptrCast(@alignCast(ctx));
        const res = self.parent_allocator.rawAlloc(len, ptr_align, ret_addr);
        if (res) |_| {
            self.metrics.alloc_count += 1;
            self.metrics.active_allocations += 1;
            self.metrics.current_allocated_bytes += len;
            self.metrics.total_allocated_bytes += len;
            if (self.metrics.current_allocated_bytes > self.metrics.peak_allocated_bytes) {
                self.metrics.peak_allocated_bytes = self.metrics.current_allocated_bytes;
            }
        }
        return res;
    }

    fn resizeFn(ctx: *anyopaque, buf: []u8, buf_align: std.mem.Alignment, new_len: usize, ret_addr: usize) bool {
        const self: *TrackedAllocator = @ptrCast(@alignCast(ctx));
        if (self.parent_allocator.rawResize(buf, buf_align, new_len, ret_addr)) {
            self.metrics.realloc_count += 1;
            if (new_len > buf.len) {
                const diff = new_len - buf.len;
                self.metrics.current_allocated_bytes += diff;
                self.metrics.total_allocated_bytes += diff;
                if (self.metrics.current_allocated_bytes > self.metrics.peak_allocated_bytes) {
                    self.metrics.peak_allocated_bytes = self.metrics.current_allocated_bytes;
                }
            } else {
                const diff = buf.len - new_len;
                self.metrics.current_allocated_bytes -= diff;
                self.metrics.total_freed_bytes += diff;
            }
            return true;
        }
        return false;
    }

    fn remapFn(ctx: *anyopaque, memory: []u8, alignment: std.mem.Alignment, new_len: usize, ret_addr: usize) ?[*]u8 {
        const self: *TrackedAllocator = @ptrCast(@alignCast(ctx));
        const res = self.parent_allocator.rawRemap(memory, alignment, new_len, ret_addr);
        if (res) |_| {
            self.metrics.realloc_count += 1;
            if (new_len > memory.len) {
                const diff = new_len - memory.len;
                self.metrics.current_allocated_bytes += diff;
                self.metrics.total_allocated_bytes += diff;
                if (self.metrics.current_allocated_bytes > self.metrics.peak_allocated_bytes) {
                    self.metrics.peak_allocated_bytes = self.metrics.current_allocated_bytes;
                }
            } else {
                const diff = memory.len - new_len;
                self.metrics.current_allocated_bytes -= diff;
                self.metrics.total_freed_bytes += diff;
            }
        }
        return res;
    }

    fn freeFn(ctx: *anyopaque, buf: []u8, buf_align: std.mem.Alignment, ret_addr: usize) void {
        const self: *TrackedAllocator = @ptrCast(@alignCast(ctx));
        self.parent_allocator.rawFree(buf, buf_align, ret_addr);
        self.metrics.free_count += 1;
        if (self.metrics.active_allocations > 0) {
            self.metrics.active_allocations -= 1;
        }
        if (self.metrics.current_allocated_bytes >= buf.len) {
            self.metrics.current_allocated_bytes -= buf.len;
        } else {
            self.metrics.current_allocated_bytes = 0;
        }
        self.metrics.total_freed_bytes += buf.len;
    }
};

/// İşletim sistemi seviyesinde RSS (Resident Set Size) okuyucu.
pub const OsRss = struct {
    /// Anlık Resident Set Size miktarını bayt cinsinden döner.
    pub fn getCurrentRssBytes() ?usize {
        switch (builtin.os.tag) {
            .linux => {
                // /proc/self/statm dosyasından 2. alan: resident page sayısı
                var buf: [128]u8 = undefined;
                const rc = std.os.linux.open("/proc/self/statm", .{ .ACCMODE = .RDONLY }, 0);
                if (@as(isize, @bitCast(rc)) < 0) return getRusageRssBytes();
                const fd: std.os.linux.fd_t = @intCast(rc);
                defer _ = std.os.linux.close(fd);

                const bytes_read = std.os.linux.read(fd, &buf, buf.len);
                if (bytes_read == 0 or @as(isize, @bitCast(bytes_read)) < 0) return getRusageRssBytes();

                const content = buf[0..bytes_read];
                var it = std.mem.tokenizeScalar(u8, content, ' ');
                _ = it.next(); // 1. program size
                if (it.next()) |resident_str| {
                    const pages = std.fmt.parseInt(usize, resident_str, 10) catch return getRusageRssBytes();
                    const page_size = std.heap.page_size_min;
                    return pages * page_size;
                }
                return getRusageRssBytes();
            },
            else => return getRusageRssBytes(),
        }
    }

    /// `getrusage` çağrısıyla zirve RSS miktarını bayt cinsinden döner.
    pub fn getRusageRssBytes() ?usize {
        switch (builtin.os.tag) {
            .linux => {
                var usage: std.os.linux.rusage = undefined;
                // Linux'ta RUSAGE_SELF = 0
                const rc = std.os.linux.getrusage(0, &usage);
                if (std.os.linux.errno(rc) == .SUCCESS) {
                    // Linux maxrss KiB cinsindendir
                    return @as(usize, @intCast(usage.maxrss)) * 1024;
                }
                return null;
            },
            .macos, .ios, .tvos, .watchos, .freebsd, .netbsd, .openbsd, .dragonfly => {
                // BSD/macOS'ta ru_maxrss bayt cinsindendir
                var usage: std.posix.rusage = undefined;
                std.posix.getrusage(std.posix.RUSAGE.SELF, &usage) catch return null;
                return @as(usize, @intCast(usage.ru_maxrss));
            },
            else => return null,
        }
    }
};

/// Olay gecikmesi (dispatch latency) anlık görüntüsü.
pub const EventLagSnapshot = struct {
    sample_count: u64 = 0,
    min_lag_ns: i128 = 0,
    max_lag_ns: i128 = 0,
    avg_lag_ns: f64 = 0,
    p50_lag_ns: i128 = 0,
    p90_lag_ns: i128 = 0,
    p99_lag_ns: i128 = 0,
};

/// Bounded sabit bellekli olay gecikmesi (Event Lag) takipçisi.
/// Sınırsız bellek harcamadan halka tampon üzerinde percentiles ve min/max/avg hesaplar.
pub const EventLagTracker = struct {
    const SAMPLE_CAPACITY: usize = 1024;

    samples: [SAMPLE_CAPACITY]i128 = @splat(0),
    sample_count: u64 = 0,
    head: usize = 0,
    total_lag_ns: u128 = 0,
    min_lag_ns: i128 = std.math.maxInt(i128),
    max_lag_ns: i128 = 0,

    pub fn init() EventLagTracker {
        return .{};
    }

    /// Olayın kuyruğa atıldığı ve işlenmeye başlandığı zaman damgalarını alarak gecikmeyi kaydeder.
    pub fn recordLag(self: *EventLagTracker, queued_at_ns: i128, dispatched_at_ns: i128) void {
        const lag = if (dispatched_at_ns >= queued_at_ns)
            dispatched_at_ns - queued_at_ns
        else
            0;
        self.recordSample(lag);
    }

    /// Doğrudan hesaplanmış gecikme nanosaniyesini kaydeder.
    pub fn recordSample(self: *EventLagTracker, lag_ns: i128) void {
        const safe_lag = if (lag_ns < 0) 0 else lag_ns;

        self.samples[self.head] = safe_lag;
        self.head = (self.head + 1) % SAMPLE_CAPACITY;
        self.sample_count += 1;
        self.total_lag_ns += @as(u128, @intCast(safe_lag));

        if (safe_lag < self.min_lag_ns) self.min_lag_ns = safe_lag;
        if (safe_lag > self.max_lag_ns) self.max_lag_ns = safe_lag;
    }

    pub fn avgLagNs(self: *const EventLagTracker) f64 {
        if (self.sample_count == 0) return 0;
        return @as(f64, @floatFromInt(self.total_lag_ns)) / @as(f64, @floatFromInt(self.sample_count));
    }

    /// Halka tampondaki mevcut örnekleri sıralayarak percentile değerini döner (p = 0.0 .. 1.0).
    pub fn calculatePercentile(self: *const EventLagTracker, p: f64) i128 {
        const count = @min(self.sample_count, SAMPLE_CAPACITY);
        if (count == 0) return 0;
        if (count == 1) return self.samples[0];

        var sorted_buf: [SAMPLE_CAPACITY]i128 = undefined;
        @memcpy(sorted_buf[0..count], self.samples[0..count]);
        std.mem.sort(i128, sorted_buf[0..count], {}, std.sort.asc(i128));

        const clamped_p = @max(0.0, @min(1.0, p));
        const index_float = clamped_p * @as(f64, @floatFromInt(count - 1));
        const index: usize = @intFromFloat(index_float);
        return sorted_buf[index];
    }

    pub fn getSnapshot(self: *const EventLagTracker) EventLagSnapshot {
        if (self.sample_count == 0) {
            return .{};
        }
        return .{
            .sample_count = self.sample_count,
            .min_lag_ns = if (self.min_lag_ns == std.math.maxInt(i128)) 0 else self.min_lag_ns,
            .max_lag_ns = self.max_lag_ns,
            .avg_lag_ns = self.avgLagNs(),
            .p50_lag_ns = self.calculatePercentile(0.50),
            .p90_lag_ns = self.calculatePercentile(0.90),
            .p99_lag_ns = self.calculatePercentile(0.99),
        };
    }
};

/// İlk kare render gecikmesi (First-Frame Render Latency) takipçisi.
pub const FirstFrameTracker = struct {
    init_timestamp_ns: i128 = 0,
    first_frame_rendered: bool = false,
    first_frame_latency_ns: ?i128 = null,

    pub fn init(now_ns: i128) FirstFrameTracker {
        return .{
            .init_timestamp_ns = now_ns,
            .first_frame_rendered = false,
            .first_frame_latency_ns = null,
        };
    }

    /// İlk karenin ekrana çizildiği anı kaydeder.
    pub fn markFirstFrame(self: *FirstFrameTracker, rendered_at_ns: i128) i128 {
        if (!self.first_frame_rendered) {
            self.first_frame_rendered = true;
            const latency = if (rendered_at_ns >= self.init_timestamp_ns)
                rendered_at_ns - self.init_timestamp_ns
            else
                0;
            self.first_frame_latency_ns = latency;
            return latency;
        }
        return self.first_frame_latency_ns orelse 0;
    }

    pub fn getLatencyNs(self: *const FirstFrameTracker) ?i128 {
        return self.first_frame_latency_ns;
    }

    pub fn getLatencyMs(self: *const FirstFrameTracker) ?f64 {
        if (self.first_frame_latency_ns) |ns| {
            return @as(f64, @floatFromInt(ns)) / @as(f64, std.time.ns_per_ms);
        }
        return null;
    }
};

/// Kapanış süresi (Shutdown Duration) takipçisi.
pub const ShutdownTracker = struct {
    shutdown_started_ns: ?i128 = null,
    shutdown_completed_ns: ?i128 = null,
    duration_ns: ?i128 = null,

    pub fn init() ShutdownTracker {
        return .{};
    }

    pub fn start(self: *ShutdownTracker, now_ns: i128) void {
        self.shutdown_started_ns = now_ns;
    }

    pub fn finish(self: *ShutdownTracker, now_ns: i128) i128 {
        self.shutdown_completed_ns = now_ns;
        if (self.shutdown_started_ns) |st| {
            const d = if (now_ns >= st) now_ns - st else 0;
            self.duration_ns = d;
            return d;
        }
        self.duration_ns = 0;
        return 0;
    }

    pub fn getDurationNs(self: *const ShutdownTracker) ?i128 {
        return self.duration_ns;
    }

    pub fn getDurationMs(self: *const ShutdownTracker) ?f64 {
        if (self.duration_ns) |ns| {
            return @as(f64, @floatFromInt(ns)) / @as(f64, std.time.ns_per_ms);
        }
        return null;
    }
};

/// Birleşik metrik anlık görüntüsü.
pub const MetricsSnapshot = struct {
    memory: MemoryMetrics,
    os_rss_bytes: ?usize,
    event_lag: EventLagSnapshot,
    first_frame_ms: ?f64,
    shutdown_ms: ?f64,

    /// Metrik özetini okunabilir bir metne biçimlendirir.
    pub fn formatSummary(self: MetricsSnapshot, buf: []u8) ![]const u8 {
        return std.fmt.bufPrint(
            buf,
            "MetricsSummary {{ RSS: {?d} B, ActiveAlloc: {d}, CurrMem: {d} B, PeakMem: {d} B, EventLagAvg: {d:.2} ns, FirstFrame: {?d:.2} ms, Shutdown: {?d:.2} ms }}",
            .{
                self.os_rss_bytes,
                self.memory.active_allocations,
                self.memory.current_allocated_bytes,
                self.memory.peak_allocated_bytes,
                self.event_lag.avg_lag_ns,
                self.first_frame_ms,
                self.shutdown_ms,
            },
        );
    }
};

/// Tüm çalışma zamanı metriklerini koordine eden ana toplayıcı.
pub const RuntimeMetricsCollector = struct {
    tracked_allocator: ?*TrackedAllocator = null,
    event_lag: EventLagTracker = EventLagTracker.init(),
    first_frame: FirstFrameTracker,
    shutdown_tracker: ShutdownTracker = ShutdownTracker.init(),

    pub fn init(now_ns: i128, tracked_alloc: ?*TrackedAllocator) RuntimeMetricsCollector {
        return .{
            .tracked_allocator = tracked_alloc,
            .event_lag = EventLagTracker.init(),
            .first_frame = FirstFrameTracker.init(now_ns),
            .shutdown_tracker = ShutdownTracker.init(),
        };
    }

    pub fn createSnapshot(self: *const RuntimeMetricsCollector) MetricsSnapshot {
        const mem = if (self.tracked_allocator) |ta| ta.getMetrics() else MemoryMetrics{};
        const rss = OsRss.getCurrentRssBytes();
        return .{
            .memory = mem,
            .os_rss_bytes = rss,
            .event_lag = self.event_lag.getSnapshot(),
            .first_frame_ms = self.first_frame.getLatencyMs(),
            .shutdown_ms = self.shutdown_tracker.getDurationMs(),
        };
    }

    /// Sağlık kontrolü: Sızıntı yoksa ve limitler aşılmamışsa true döner.
    pub fn isHealthy(self: *const RuntimeMetricsCollector) bool {
        if (self.tracked_allocator) |ta| {
            if (ta.metrics.hasLeaks()) return false;
        }
        return true;
    }
};

test "tracked allocator bellek tahsis, yeniden boyutlandirma ve sizinti takibi" {
    var tracker = TrackedAllocator.init(std.testing.allocator);
    const alloc = tracker.allocator();

    // 1. Tahsis yap
    const slice = try alloc.alloc(u8, 256);
    try std.testing.expectEqual(@as(u64, 1), tracker.metrics.alloc_count);
    try std.testing.expectEqual(@as(usize, 256), tracker.metrics.current_allocated_bytes);
    try std.testing.expectEqual(@as(usize, 256), tracker.metrics.peak_allocated_bytes);
    try std.testing.expectEqual(@as(usize, 1), tracker.metrics.active_allocations);
    try std.testing.expect(tracker.metrics.hasLeaks());

    // 2. Yeniden boyutlandır (büyüt)
    const resized = alloc.realloc(slice, 512) catch slice;
    try std.testing.expect(resized.len == 512 or resized.len == 256);
    try std.testing.expect(tracker.metrics.peak_allocated_bytes >= 256);

    // 3. Serbest bırak
    alloc.free(resized);
    try std.testing.expectEqual(@as(usize, 0), tracker.metrics.current_allocated_bytes);
    try std.testing.expectEqual(@as(usize, 0), tracker.metrics.active_allocations);
    try std.testing.expect(!tracker.metrics.hasLeaks());
    try tracker.assertNoLeaks();
}

test "event lag tracker min, max, avg ve percentiles hesabi" {
    var elt = EventLagTracker.init();

    elt.recordLag(1000, 1050); // 50ns
    elt.recordLag(2000, 2100); // 100ns
    elt.recordLag(3000, 3150); // 150ns
    elt.recordLag(4000, 4200); // 200ns

    try std.testing.expectEqual(@as(u64, 4), elt.sample_count);
    try std.testing.expectEqual(@as(i128, 50), elt.min_lag_ns);
    try std.testing.expectEqual(@as(i128, 200), elt.max_lag_ns);
    try std.testing.expectEqual(@as(f64, 125.0), elt.avgLagNs());

    const snap = elt.getSnapshot();
    try std.testing.expectEqual(@as(i128, 50), snap.min_lag_ns);
    try std.testing.expectEqual(@as(i128, 200), snap.max_lag_ns);
    try std.testing.expect(snap.p50_lag_ns >= 100);
    try std.testing.expect(snap.p99_lag_ns >= 150);
}

test "first frame tracker ve shutdown tracker zaman takibi" {
    var fft = FirstFrameTracker.init(1_000_000);
    try std.testing.expect(!fft.first_frame_rendered);
    try std.testing.expect(fft.getLatencyNs() == null);

    const lat = fft.markFirstFrame(1_050_000);
    try std.testing.expectEqual(@as(i128, 50_000), lat);
    try std.testing.expect(fft.first_frame_rendered);
    try std.testing.expectEqual(@as(i128, 50_000), fft.getLatencyNs().?);

    // İkinci çağrı aynı latans değerini korumalı (idempotence)
    const lat2 = fft.markFirstFrame(2_000_000);
    try std.testing.expectEqual(@as(i128, 50_000), lat2);

    var sdt = ShutdownTracker.init();
    sdt.start(10_000_000);
    const s_dur = sdt.finish(10_025_000);
    try std.testing.expectEqual(@as(i128, 25_000), s_dur);
    try std.testing.expectEqual(@as(i128, 25_000), sdt.getDurationNs().?);
    try std.testing.expect(sdt.getDurationMs().? > 0.0);
}

test "os rss okuyucu calisir ve gecerli deger doner" {
    const rss = OsRss.getCurrentRssBytes();
    if (rss) |val| {
        try std.testing.expect(val > 0);
    }
    const peak = OsRss.getRusageRssBytes();
    if (peak) |val| {
        try std.testing.expect(val > 0);
    }
}

test "runtime metrics collector ve formatSummary" {
    var tracker = TrackedAllocator.init(std.testing.allocator);
    var collector = RuntimeMetricsCollector.init(1_000_000, &tracker);

    collector.event_lag.recordLag(100, 250);
    _ = collector.first_frame.markFirstFrame(1_020_000);
    collector.shutdown_tracker.start(2_000_000);
    _ = collector.shutdown_tracker.finish(2_005_000);

    try std.testing.expect(collector.isHealthy());

    const snap = collector.createSnapshot();
    try std.testing.expectEqual(@as(u64, 1), snap.event_lag.sample_count);
    try std.testing.expect(snap.first_frame_ms != null);
    try std.testing.expect(snap.shutdown_ms != null);

    var buf: [512]u8 = undefined;
    const summary = try snap.formatSummary(&buf);
    try std.testing.expect(summary.len > 0);
    try std.testing.expect(std.mem.indexOf(u8, summary, "MetricsSummary") != null);
}

test "memory metrics isFlat metodu dogruluk testi" {
    var m = MemoryMetrics{
        .current_allocated_bytes = 1024,
    };
    try std.testing.expect(m.isFlat(1024, 0));
    try std.testing.expect(m.isFlat(1000, 50));
    try std.testing.expect(!m.isFlat(1000, 10));
}
