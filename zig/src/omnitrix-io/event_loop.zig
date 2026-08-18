//! Çapraz platform Event Loop ve Deterministik Boşaltma (tasarım Bölüm 3.1).
//!
//! - Linux: EpollBackend
//! - macOS/BSD: KqueueBackend
//! - Windows: IocpBackend
//! - Monotonic deadline tabanlı zamanlayıcı kuyruğu (timers)
//! - Bounded user-event kuyruğu ve uyandırma kanalı (wakeup)
//! - Shutdown sırasında bekleyen operasyonların deterministik boşaltılması (drainage)
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");
const builtin = @import("builtin");
const types = @import("types.zig");
const deadline_mod = @import("deadline.zig");
const cancel_mod = @import("cancel.zig");
const queue_mod = @import("queue.zig");
const epoll = @import("epoll.zig");
const kqueue = @import("kqueue.zig");
const iocp = @import("iocp.zig");

pub const Interest = types.Interest;
pub const ReadyEvents = types.ReadyEvents;
pub const IoEvent = types.IoEvent;
pub const TimerEntry = types.TimerEntry;
pub const Deadline = deadline_mod.Deadline;
pub const Monotonic = deadline_mod.Monotonic;
pub const CancellationToken = cancel_mod.CancellationToken;
pub const BoundedQueue = queue_mod.BoundedQueue;

pub const EventLoopError = error{
    BackendInitFailed,
    RegistrationFailed,
    ModifyFailed,
    UnregisterFailed,
    WakeupFailed,
    PollFailed,
    TimerQueueFull,
    EventQueueFull,
    LoopShuttingDown,
    InvalidCapacity,
};

/// Event loop çıktısı: IO olayı, zamanlayıcı veya kullanıcı mesajı.
pub const LoopEvent = union(enum) {
    io: IoEvent,
    timer: struct {
        id: u64,
        userdata: usize,
    },
    user: usize,
};

pub const EventLoop = struct {
    const PlatformBackend = switch (builtin.os.tag) {
        .linux => epoll.EpollBackend,
        .macos, .freebsd, .netbsd, .openbsd, .dragonfly => kqueue.KqueueBackend,
        .windows => iocp.IocpBackend,
        else => epoll.EpollBackend,
    };

    allocator: std.mem.Allocator,
    backend: PlatformBackend,
    timers: std.ArrayList(TimerEntry),
    user_queue: BoundedQueue(usize),
    next_timer_id: u64 = 1,
    is_shutting_down: bool = false,
    now_fn: *const fn () i128 = Monotonic.now,

    pub fn init(allocator: std.mem.Allocator, max_user_events: usize) !EventLoop {
        var be = PlatformBackend.init(allocator) catch return error.BackendInitFailed;
        errdefer be.deinit();

        var q = BoundedQueue(usize).init(allocator, max_user_events) catch return error.InvalidCapacity;
        errdefer q.deinit();

        return .{
            .allocator = allocator,
            .backend = be,
            .timers = std.ArrayList(TimerEntry).empty,
            .user_queue = q,
            .is_shutting_down = false,
            .now_fn = Monotonic.now,
        };
    }

    pub fn deinit(self: *EventLoop) void {
        self.timers.deinit(self.allocator);
        self.user_queue.deinit();
        self.backend.deinit();
        self.* = undefined;
    }

    pub fn register(self: *EventLoop, fd: i32, interest: Interest, userdata: usize) EventLoopError!void {
        if (self.is_shutting_down) return error.LoopShuttingDown;
        self.backend.register(fd, interest, userdata) catch return error.RegistrationFailed;
    }

    pub fn modify(self: *EventLoop, fd: i32, interest: Interest, userdata: usize) EventLoopError!void {
        if (self.is_shutting_down) return error.LoopShuttingDown;
        self.backend.modify(fd, interest, userdata) catch return error.ModifyFailed;
    }

    pub fn unregister(self: *EventLoop, fd: i32) EventLoopError!void {
        self.backend.unregister(fd) catch return error.UnregisterFailed;
    }

    /// Zamanlayıcı ekler. Belirtilen mutlak monotonic nanosaniyede tetiklenir.
    pub fn addTimer(self: *EventLoop, deadline: Deadline, userdata: usize) EventLoopError!u64 {
        if (self.is_shutting_down) return error.LoopShuttingDown;
        const id = self.next_timer_id;
        self.next_timer_id += 1;

        self.timers.append(self.allocator, .{
            .id = id,
            .deadline_ns = deadline.deadline_ns,
            .userdata = userdata,
            .cancelled = false,
        }) catch return error.TimerQueueFull;

        return id;
    }

    /// Zamanlayıcıyı iptal eder.
    pub fn cancelTimer(self: *EventLoop, id: u64) bool {
        for (self.timers.items) |*t| {
            if (t.id == id and !t.cancelled) {
                t.cancelled = true;
                return true;
            }
        }
        return false;
    }

    /// Başka bir iş parçacığı veya operasyondan olay kuyruğuna mesaj bırakır.
    pub fn post(self: *EventLoop, userdata: usize) EventLoopError!void {
        if (self.is_shutting_down) return error.LoopShuttingDown;
        self.user_queue.push(userdata) catch return error.EventQueueFull;
        self.backend.wakeup() catch return error.WakeupFailed;
    }

    /// Sıradaki en yakın zamanlayıcıya kadar kalan süreyi hesaplar (ms cinsinden).
    fn calculateTimeoutMs(self: *const EventLoop, now: i128, max_timeout_ns: ?i128) i32 {
        var min_remaining: i128 = if (max_timeout_ns) |m| m else std.math.maxInt(i128);

        for (self.timers.items) |t| {
            if (t.cancelled) continue;
            const rem = t.deadline_ns - now;
            if (rem <= 0) return 0; // Zamanı dolmuş zamanlayıcı var, bekleme
            if (rem < min_remaining) min_remaining = rem;
        }

        if (min_remaining == std.math.maxInt(i128)) {
            return -1; // Sonsuz bekleme
        }

        const ms = @divTrunc(min_remaining, std.time.ns_per_ms);
        if (ms <= 0) return 0;
        if (ms > std.math.maxInt(i32)) return std.math.maxInt(i32);
        return @intCast(ms);
    }

    /// Tek bir döngü adımı çalıştırır ve gerçekleşen olayları out_events içine yazar.
    pub fn poll(
        self: *EventLoop,
        out_events: []LoopEvent,
        max_timeout_ns: ?i128,
    ) EventLoopError!usize {
        if (out_events.len == 0) return 0;
        var out_idx: usize = 0;

        // 1. Önce bekleyen kullanıcı olaylarını boşalt
        while (out_idx < out_events.len and !self.user_queue.isEmpty()) {
            const user_msg = self.user_queue.pop() catch break;
            out_events[out_idx] = .{ .user = user_msg };
            out_idx += 1;
        }
        if (out_idx >= out_events.len) return out_idx;

        // 2. Süresi dolan zamanlayıcıları tara
        const now = self.now_fn();
        var timer_idx: usize = 0;
        while (timer_idx < self.timers.items.len and out_idx < out_events.len) {
            const t = self.timers.items[timer_idx];
            if (t.cancelled) {
                _ = self.timers.swapRemove(timer_idx);
                continue;
            }
            if (t.deadline_ns <= now) {
                out_events[out_idx] = .{
                    .timer = .{
                        .id = t.id,
                        .userdata = t.userdata,
                    },
                };
                out_idx += 1;
                _ = self.timers.swapRemove(timer_idx);
                continue;
            }
            timer_idx += 1;
        }
        if (out_idx >= out_events.len) return out_idx;

        // 3. Platform backend poll
        const timeout_ms = self.calculateTimeoutMs(now, max_timeout_ns);
        var raw_io_events: [32]IoEvent = undefined;
        const max_io = @min(raw_io_events.len, out_events.len - out_idx);

        const io_count = self.backend.poll(raw_io_events[0..max_io], timeout_ms) catch return error.PollFailed;
        for (raw_io_events[0..io_count]) |ev| {
            out_events[out_idx] = .{ .io = ev };
            out_idx += 1;
        }

        return out_idx;
    }

    /// Non-blocking deterministik boşaltma (drainage).
    /// Bekleyen tüm zamanlayıcıları ve kullanıcı kuyruğunu sıfır timeout ile tüketir.
    pub fn drain(self: *EventLoop, out_events: []LoopEvent) usize {
        return self.poll(out_events, 0) catch 0;
    }

    /// Event loop'u deterministik olarak sonlandırır.
    /// Kalan kuyrukları temizler, platform kaynaklarını serbest bırakır.
    pub fn shutdown(self: *EventLoop) void {
        self.is_shutting_down = true;
        self.timers.clearRetainingCapacity();
        self.user_queue.clear();
    }
};

test "event loop init, timer ve poll" {
    var loop = try EventLoop.init(std.testing.allocator, 16);
    defer loop.deinit();

    const FakeClock = struct {
        var now_val: i128 = 1_000_000;
        fn f() i128 {
            return now_val;
        }
    };
    loop.now_fn = FakeClock.f;

    // 50ms ve 100ms zamanlayıcıları ekle
    const d1 = Deadline.fromMillis(FakeClock.f, 50);
    const d2 = Deadline.fromMillis(FakeClock.f, 100);
    const t1 = try loop.addTimer(d1, 101);
    const t2 = try loop.addTimer(d2, 102);

    var events: [8]LoopEvent = undefined;

    // Saat henüz ilerlemedi
    var n = try loop.poll(&events, 0);
    try std.testing.expectEqual(@as(usize, 0), n);

    // 60ms ilerlet -> t1 dolmalı, t2 henüz değil
    FakeClock.now_val += 60 * std.time.ns_per_ms;
    n = try loop.poll(&events, 0);
    try std.testing.expectEqual(@as(usize, 1), n);
    try std.testing.expectEqual(t1, events[0].timer.id);
    try std.testing.expectEqual(@as(usize, 101), events[0].timer.userdata);

    // t2 iptal et
    try std.testing.expect(loop.cancelTimer(t2));
    FakeClock.now_val += 60 * std.time.ns_per_ms;
    n = try loop.poll(&events, 0);
    try std.testing.expectEqual(@as(usize, 0), n);
}

test "event loop post user events ve drainage" {
    var loop = try EventLoop.init(std.testing.allocator, 8);
    defer loop.deinit();

    try loop.post(555);
    try loop.post(777);

    var events: [8]LoopEvent = undefined;
    const n = loop.drain(&events);
    try std.testing.expectEqual(@as(usize, 2), n);
    try std.testing.expectEqual(@as(usize, 555), events[0].user);
    try std.testing.expectEqual(@as(usize, 777), events[1].user);
}

test "event loop shutdown deterministik bosaltma" {
    var loop = try EventLoop.init(std.testing.allocator, 8);
    defer loop.deinit();

    const d = Deadline.fromSeconds(loop.now_fn, 10);
    _ = try loop.addTimer(d, 999);
    try loop.post(1234);

    loop.shutdown();
    try std.testing.expect(loop.is_shutting_down);
    try std.testing.expectError(error.LoopShuttingDown, loop.post(5678));
}
