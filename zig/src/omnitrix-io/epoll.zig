//! Linux epoll backend (tasarım Bölüm 3.1).
//!
//! Linux çekirdeğinin epoll syscall katmanını doğrudan kullanır.
//! std.Io.Evented'e bağımlı değildir.

const std = @import("std");
const builtin = @import("builtin");
const types = @import("types.zig");

pub const IoEvent = types.IoEvent;
pub const Interest = types.Interest;
pub const ReadyEvents = types.ReadyEvents;

pub const EpollError = error{
    EpollCreateFailed,
    EventFdCreateFailed,
    RegistrationFailed,
    ModifyFailed,
    UnregisterFailed,
    WakeupFailed,
    PollFailed,
};

pub const EpollBackend = struct {
    epoll_fd: i32,
    wakeup_fd: i32,
    allocator: std.mem.Allocator,

    const WAKEUP_USERDATA: usize = std.math.maxInt(usize);

    pub fn init(allocator: std.mem.Allocator) EpollError!EpollBackend {
        if (builtin.os.tag != .linux) {
            return error.EpollCreateFailed;
        }

        const rc_epfd = std.os.linux.epoll_create1(std.os.linux.EPOLL.CLOEXEC);
        const epfd: i32 = @intCast(rc_epfd);
        if (epfd < 0) return error.EpollCreateFailed;
        errdefer _ = std.os.linux.close(epfd);

        const rc_efd = std.os.linux.eventfd(0, std.os.linux.EFD.CLOEXEC | std.os.linux.EFD.NONBLOCK);
        const efd: i32 = @intCast(rc_efd);
        if (efd < 0) return error.EventFdCreateFailed;
        errdefer _ = std.os.linux.close(efd);

        const self = EpollBackend{
            .epoll_fd = epfd,
            .wakeup_fd = efd,
            .allocator = allocator,
        };

        // Wakeup fd'yi epoll'e ekle
        var ev = std.os.linux.epoll_event{
            .events = std.os.linux.EPOLL.IN,
            .data = .{ .u64 = WAKEUP_USERDATA },
        };
        const ctl_res = std.os.linux.epoll_ctl(epfd, std.os.linux.EPOLL.CTL_ADD, efd, &ev);
        if (std.os.linux.errno(ctl_res) != .SUCCESS) {
            return error.RegistrationFailed;
        }

        return self;
    }

    pub fn deinit(self: *EpollBackend) void {
        if (self.epoll_fd >= 0) {
            _ = std.os.linux.close(self.epoll_fd);
            self.epoll_fd = -1;
        }
        if (self.wakeup_fd >= 0) {
            _ = std.os.linux.close(self.wakeup_fd);
            self.wakeup_fd = -1;
        }
        self.* = undefined;
    }

    fn interestToEpollFlags(interest: Interest) u32 {
        var flags: u32 = 0;
        if (interest.readable) flags |= std.os.linux.EPOLL.IN;
        if (interest.writable) flags |= std.os.linux.EPOLL.OUT;
        if (interest.edge_triggered) flags |= std.os.linux.EPOLL.ET;
        if (interest.one_shot) flags |= std.os.linux.EPOLL.ONESHOT;
        return flags;
    }

    pub fn register(self: *EpollBackend, fd: i32, interest: Interest, userdata: usize) EpollError!void {
        var ev = std.os.linux.epoll_event{
            .events = interestToEpollFlags(interest),
            .data = .{ .u64 = userdata },
        };
        const res = std.os.linux.epoll_ctl(self.epoll_fd, std.os.linux.EPOLL.CTL_ADD, fd, &ev);
        if (std.os.linux.errno(res) != .SUCCESS) {
            return error.RegistrationFailed;
        }
    }

    pub fn modify(self: *EpollBackend, fd: i32, interest: Interest, userdata: usize) EpollError!void {
        var ev = std.os.linux.epoll_event{
            .events = interestToEpollFlags(interest),
            .data = .{ .u64 = userdata },
        };
        const res = std.os.linux.epoll_ctl(self.epoll_fd, std.os.linux.EPOLL.CTL_MOD, fd, &ev);
        if (std.os.linux.errno(res) != .SUCCESS) {
            return error.ModifyFailed;
        }
    }

    pub fn unregister(self: *EpollBackend, fd: i32) EpollError!void {
        const res = std.os.linux.epoll_ctl(self.epoll_fd, std.os.linux.EPOLL.CTL_DEL, fd, null);
        if (std.os.linux.errno(res) != .SUCCESS) {
            return error.UnregisterFailed;
        }
    }

    /// Event loop'u diğer bir iş parçacığından veya callback'ten hemen uyandırır.
    pub fn wakeup(self: *const EpollBackend) EpollError!void {
        const val: u64 = 1;
        const res = std.os.linux.write(self.wakeup_fd, std.mem.asBytes(&val), @sizeOf(u64));
        if (res != @sizeOf(u64)) {
            return error.WakeupFailed;
        }
    }

    /// Uyandırma sinyalini okur ve tamponu temizler.
    fn consumeWakeup(self: *const EpollBackend) void {
        var val: u64 = 0;
        _ = std.os.linux.read(self.wakeup_fd, std.mem.asBytes(&val), @sizeOf(u64));
    }

    /// Hazır olayları bekler ve out_events dilimine yazar.
    /// timeout_ms: < 0 ise sonsuz, 0 ise anında yoklama (non-blocking), > 0 ise milisaniye.
    /// Dönen değer: gerçekleşen olay sayısı.
    pub fn poll(self: *EpollBackend, out_events: []IoEvent, timeout_ms: i32) EpollError!usize {
        var raw_events: [64]std.os.linux.epoll_event = undefined;
        const max_ev = @min(raw_events.len, out_events.len);
        if (max_ev == 0) return 0;

        const count = std.os.linux.epoll_wait(self.epoll_fd, &raw_events, @intCast(max_ev), timeout_ms);
        if (count < 0) {
            const err = std.os.linux.errno(count);
            if (err == .INTR) return 0; // Sinyal kesintisi — hata değil
            return error.PollFailed;
        }

        var out_idx: usize = 0;
        const n: usize = @intCast(count);
        for (raw_events[0..n]) |raw| {
            if (raw.data.u64 == WAKEUP_USERDATA) {
                self.consumeWakeup();
                continue;
            }

            var ready = ReadyEvents{};
            if ((raw.events & std.os.linux.EPOLL.IN) != 0) ready.readable = true;
            if ((raw.events & std.os.linux.EPOLL.OUT) != 0) ready.writable = true;
            if ((raw.events & std.os.linux.EPOLL.ERR) != 0) ready.is_error = true;
            if ((raw.events & std.os.linux.EPOLL.HUP) != 0) ready.is_hangup = true;

            out_events[out_idx] = .{
                .fd = -1, // Epoll data u64 userdata taşır
                .events = ready,
                .userdata = @intCast(raw.data.u64),
            };
            out_idx += 1;
        }

        return out_idx;
    }
};

test "epoll backend kayit, wakeup ve poll calisir" {
    if (builtin.os.tag != .linux) return;

    var ep = try EpollBackend.init(std.testing.allocator);
    defer ep.deinit();

    // Wakeup testi
    try ep.wakeup();

    var events: [8]IoEvent = undefined;
    const n = try ep.poll(&events, 100);
    // Wakeup tüketildiği için normal olay listesine aktarılmaz, return 0 olur
    try std.testing.expectEqual(@as(usize, 0), n);
}

test "epoll eventfd kayit ve okuma" {
    if (builtin.os.tag != .linux) return;

    var ep = try EpollBackend.init(std.testing.allocator);
    defer ep.deinit();

    const rc = std.os.linux.eventfd(0, std.os.linux.EFD.CLOEXEC | std.os.linux.EFD.NONBLOCK);
    const test_fd: i32 = @intCast(rc);
    try std.testing.expect(test_fd >= 0);
    defer _ = std.os.linux.close(test_fd);

    try ep.register(test_fd, Interest.read_only, 42);

    // Henüz veri yazılmadı, poll boş dönmeli
    var events: [8]IoEvent = undefined;
    var n = try ep.poll(&events, 0);
    try std.testing.expectEqual(@as(usize, 0), n);

    // Veri yaz
    const val: u64 = 1;
    _ = std.os.linux.write(test_fd, std.mem.asBytes(&val), @sizeOf(u64));

    n = try ep.poll(&events, 100);
    try std.testing.expectEqual(@as(usize, 1), n);
    try std.testing.expectEqual(@as(usize, 42), events[0].userdata);
    try std.testing.expect(events[0].events.readable);

    // Unregister
    try ep.unregister(test_fd);
}
