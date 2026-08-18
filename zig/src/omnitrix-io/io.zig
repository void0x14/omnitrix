//! omnitrix-io (tasarım Bölüm 3.1).
//!
//! Platform event loop ve dosya/socket IO katmanının çekirdek sözleşmeleri:
//! monotonic deadline, cancellation token, bounded queue, platform event loop
//! (Linux epoll, macOS/BSD kqueue, Windows IOCP) ve shutdown sırasında bekleyen
//! operasyonların deterministik boşaltılması.
//!
//! Bu modül upstream `std.Io.Evented` durumuna bağımlı DEĞİLDİR (tasarım 3.1).

const std = @import("std");

pub const deadline = @import("deadline.zig");
pub const cancel = @import("cancel.zig");
pub const queue = @import("queue.zig");
pub const types = @import("types.zig");
pub const epoll = @import("epoll.zig");
pub const kqueue = @import("kqueue.zig");
pub const iocp = @import("iocp.zig");
pub const event_loop = @import("event_loop.zig");

pub const Deadline = deadline.Deadline;
pub const Monotonic = deadline.Monotonic;
pub const DeadlineError = deadline.DeadlineError;

pub const CancellationToken = cancel.CancellationToken;
pub const CancelError = cancel.CancelError;

pub const BoundedQueue = queue.BoundedQueue;
pub const QueueError = queue.QueueError;

pub const Interest = types.Interest;
pub const ReadyEvents = types.ReadyEvents;
pub const IoEvent = types.IoEvent;
pub const TimerEntry = types.TimerEntry;

pub const EventLoop = event_loop.EventLoop;
pub const EventLoopError = event_loop.EventLoopError;
pub const LoopEvent = event_loop.LoopEvent;

test {
    std.testing.refAllDecls(@This());
}
