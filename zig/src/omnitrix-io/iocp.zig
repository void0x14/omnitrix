//! Windows IOCP backend sözleşmesi (tasarım Bölüm 3.1, 3.3).
//!
//! Windows I/O Completion Ports (IOCP) altyapı sözleşmesi.
//! Ortak API platform farklarını gizler.

const std = @import("std");
const builtin = @import("builtin");
const types = @import("types.zig");

pub const IoEvent = types.IoEvent;
pub const Interest = types.Interest;
pub const ReadyEvents = types.ReadyEvents;

pub const IocpError = error{
    IocpCreateFailed,
    RegistrationFailed,
    ModifyFailed,
    UnregisterFailed,
    WakeupFailed,
    PollFailed,
    UnsupportedPlatform,
};

pub const IocpBackend = struct {
    iocp_handle: usize = 0,
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator) IocpError!IocpBackend {
        return .{
            .iocp_handle = 0,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *IocpBackend) void {
        self.* = undefined;
    }

    pub fn register(self: *IocpBackend, fd: i32, interest: Interest, userdata: usize) IocpError!void {
        _ = self;
        _ = fd;
        _ = interest;
        _ = userdata;
    }

    pub fn modify(self: *IocpBackend, fd: i32, interest: Interest, userdata: usize) IocpError!void {
        _ = self;
        _ = fd;
        _ = interest;
        _ = userdata;
    }

    pub fn unregister(self: *IocpBackend, fd: i32) IocpError!void {
        _ = self;
        _ = fd;
    }

    pub fn wakeup(self: *const IocpBackend) IocpError!void {
        _ = self;
    }

    pub fn poll(self: *IocpBackend, out_events: []IoEvent, timeout_ms: i32) IocpError!usize {
        _ = self;
        _ = out_events;
        _ = timeout_ms;
        return 0;
    }
};

test "iocp backend init/deinit sozlesmesi" {
    var iocp = try IocpBackend.init(std.testing.allocator);
    defer iocp.deinit();

    try iocp.register(1, Interest.read_only, 200);
    try iocp.modify(1, Interest.read_write, 200);
    try iocp.unregister(1);
    try iocp.wakeup();

    var events: [4]IoEvent = undefined;
    const n = try iocp.poll(&events, 0);
    try std.testing.expectEqual(@as(usize, 0), n);
}
