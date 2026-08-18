//! Ortak IO tipleri ve olay modelleri (tasarım Bölüm 3.1).
//!
//! Platform farklarını gizleyen ortak olay ve kayıt tipleri.
//! Epoll (Linux), Kqueue (macOS/BSD) ve IOCP (Windows) bu ortak sözleşmeyi uygular.

const std = @import("std");

/// Olay dinleme modu / ilgi alanları.
pub const Interest = packed struct(u8) {
    readable: bool = false,
    writable: bool = false,
    edge_triggered: bool = false,
    one_shot: bool = false,
    _padding: u4 = 0,

    pub const read_only: Interest = .{ .readable = true };
    pub const write_only: Interest = .{ .writable = true };
    pub const read_write: Interest = .{ .readable = true, .writable = true };
};

/// Gerçekleşen IO olayları.
pub const ReadyEvents = packed struct(u8) {
    readable: bool = false,
    writable: bool = false,
    is_error: bool = false,
    is_hangup: bool = false,
    _padding: u4 = 0,
};

/// Ortak IO olay kaydı.
pub const IoEvent = struct {
    fd: i32,
    events: ReadyEvents,
    userdata: usize = 0,
};

/// Zamanlayıcı kaydı (monotonic deadline ile).
pub const TimerEntry = struct {
    id: u64,
    deadline_ns: i128,
    userdata: usize = 0,
    cancelled: bool = false,
};

test "interest and ready events flags" {
    const int = Interest.read_write;
    try std.testing.expect(int.readable);
    try std.testing.expect(int.writable);
    try std.testing.expect(!int.edge_triggered);

    const ready = ReadyEvents{ .readable = true, .is_error = false };
    try std.testing.expect(ready.readable);
    try std.testing.expect(!ready.is_error);
}
