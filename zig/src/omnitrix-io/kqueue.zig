//! macOS/BSD kqueue backend sözleşmesi (tasarım Bölüm 3.1, 3.3).
//!
//! macOS ve BSD platformları için kqueue/kevent altyapısı.
//! Ortak API platform farklarını gizler.

const std = @import("std");
const builtin = @import("builtin");
const types = @import("types.zig");

pub const IoEvent = types.IoEvent;
pub const Interest = types.Interest;
pub const ReadyEvents = types.ReadyEvents;

pub const KqueueError = error{
    KqueueCreateFailed,
    RegistrationFailed,
    ModifyFailed,
    UnregisterFailed,
    WakeupFailed,
    PollFailed,
    UnsupportedPlatform,
};

pub const KqueueBackend = struct {
    kq_fd: i32 = -1,
    wakeup_pipe: [2]i32 = .{ -1, -1 },
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator) KqueueError!KqueueBackend {
        if (builtin.os.tag == .macos or builtin.os.tag.isBSD()) {
            // BSD / macOS'ta kqueue syscall'i açılır
            const fd = std.posix.kqueue() catch return error.KqueueCreateFailed;
            return .{
                .kq_fd = fd,
                .allocator = allocator,
            };
        } else {
            // Linux veya diğer platformlarda test ve çapraz derleme stub'ı
            return .{
                .kq_fd = -1,
                .allocator = allocator,
            };
        }
    }

    fn closeFd(fd: i32) void {
        if (builtin.os.tag == .linux) {
            _ = std.os.linux.close(fd);
        } else if (@hasDecl(std.c, "close")) {
            _ = std.c.close(fd);
        }
    }

    pub fn deinit(self: *KqueueBackend) void {
        if (self.kq_fd >= 0) {
            closeFd(self.kq_fd);
            self.kq_fd = -1;
        }
        if (self.wakeup_pipe[0] >= 0) {
            closeFd(self.wakeup_pipe[0]);
            self.wakeup_pipe[0] = -1;
        }
        if (self.wakeup_pipe[1] >= 0) {
            closeFd(self.wakeup_pipe[1]);
            self.wakeup_pipe[1] = -1;
        }
        self.* = undefined;
    }

    pub fn register(self: *KqueueBackend, fd: i32, interest: Interest, userdata: usize) KqueueError!void {
        _ = self;
        _ = fd;
        _ = interest;
        _ = userdata;
        if (builtin.os.tag != .macos and !builtin.os.tag.isBSD()) {
            return;
        }
    }

    pub fn modify(self: *KqueueBackend, fd: i32, interest: Interest, userdata: usize) KqueueError!void {
        _ = self;
        _ = fd;
        _ = interest;
        _ = userdata;
        if (builtin.os.tag != .macos and !builtin.os.tag.isBSD()) {
            return;
        }
    }

    pub fn unregister(self: *KqueueBackend, fd: i32) KqueueError!void {
        _ = self;
        _ = fd;
        if (builtin.os.tag != .macos and !builtin.os.tag.isBSD()) {
            return;
        }
    }

    pub fn wakeup(self: *const KqueueBackend) KqueueError!void {
        _ = self;
    }

    pub fn poll(self: *KqueueBackend, out_events: []IoEvent, timeout_ms: i32) KqueueError!usize {
        _ = self;
        _ = out_events;
        _ = timeout_ms;
        return 0;
    }
};

test "kqueue backend init/deinit sozlesmesi" {
    var kq = try KqueueBackend.init(std.testing.allocator);
    defer kq.deinit();

    try kq.register(1, Interest.read_only, 100);
    try kq.modify(1, Interest.read_write, 100);
    try kq.unregister(1);
    try kq.wakeup();

    var events: [4]IoEvent = undefined;
    const n = try kq.poll(&events, 0);
    try std.testing.expectEqual(@as(usize, 0), n);
}
