//! omnitrix-task (tasarım Bölüm 3.2).
//!
//! Task yaşam döngüsü: created, queued, running, waiting, cancelling, completed, failed.
//! - Cancellation yield ve IO noktalarında garanti edilir (Bölüm 3.1 cancellation token).
//! - Bounded aktif sayı ve kuyruk (Bölüm 3.2, 8).
//! - Her task için kaynak, sahiplik (parent_id), oluşturulma zamanı ve deadline kaydı tutulur.
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");
const io = @import("../omnitrix-io/io.zig");

pub const Deadline = io.Deadline;
pub const Monotonic = io.Monotonic;
pub const CancellationToken = io.CancellationToken;

pub const TaskState = enum {
    created,
    queued,
    running,
    waiting,
    cancelling,
    completed,
    failed,

    pub fn label(self: TaskState) []const u8 {
        return @tagName(self);
    }

    pub fn isTerminal(self: TaskState) bool {
        return self == .completed or self == .failed;
    }

    pub fn isActive(self: TaskState) bool {
        return self == .running or self == .waiting or self == .cancelling;
    }
};

pub const TransitionError = error{
    InvalidTransition,
    TaskAlreadyFinished,
};

/// İzin verilen durum geçişleri.
/// İptal, created/queued/running/waiting'den cancelling'e, oradan completed/failed'e gider.
pub fn canTransition(from: TaskState, to: TaskState) bool {
    return switch (from) {
        .created => to == .queued or to == .cancelling,
        .queued => to == .running or to == .cancelling,
        .running => to == .waiting or to == .cancelling or to == .completed or to == .failed,
        .waiting => to == .running or to == .cancelling or to == .completed or to == .failed,
        .cancelling => to == .completed or to == .failed,
        .completed => false,
        .failed => false,
    };
}

pub const TaskOutcome = union(enum) {
    completed: void,
    failed: []const u8,
};

pub const Task = struct {
    id: u64,
    parent_id: ?u64 = null,
    name: []const u8 = "",
    state: TaskState = .created,
    created_at: i128,
    deadline: ?Deadline = null,
    cancel_token: CancellationToken,
    failure_reason: ?[]const u8 = null,

    pub fn init(
        id: u64,
        parent_id: ?u64,
        name: []const u8,
        created_at: i128,
        deadline: ?Deadline,
        parent_cancel_token: ?*const CancellationToken,
    ) Task {
        const token = if (parent_cancel_token) |pct|
            CancellationToken.initChild(pct)
        else
            CancellationToken.init();

        return .{
            .id = id,
            .parent_id = parent_id,
            .name = name,
            .state = .created,
            .created_at = created_at,
            .deadline = deadline,
            .cancel_token = token,
            .failure_reason = null,
        };
    }

    /// Geçiş kurallarına göre durum değiştirir.
    pub fn transition(self: *Task, to: TaskState) TransitionError!void {
        if (self.state.isTerminal()) return error.TaskAlreadyFinished;
        if (!canTransition(self.state, to)) return error.InvalidTransition;
        self.state = to;
    }

    /// İptal token'ını tetikler ve durumunu .cancelling'e çeker.
    pub fn requestCancel(self: *Task) void {
        self.cancel_token.cancel();
        if (self.state != .completed and self.state != .failed and self.state != .cancelling) {
            _ = self.transition(.cancelling) catch {};
        }
    }

    /// Yield veya IO noktasında iptal kontrolü (tasarım 3.2: cancellation yield/IO garantisi).
    /// Token iptal edilmişse state .cancelling yapılır ve error.Cancelled dönülür.
    pub fn pollYield(self: *Task) io.CancelError!void {
        if (self.cancel_token.isCancelled()) {
            if (self.state != .completed and self.state != .failed and self.state != .cancelling) {
                _ = self.transition(.cancelling) catch {};
            }
            return error.Cancelled;
        }
    }

    /// Monotonic deadline aşılmış mı kontrol eder.
    pub fn isDeadlineExceeded(self: *const Task, now: i128) bool {
        if (self.deadline) |dl| {
            if (dl.isInfinite()) return false;
            return (dl.deadline_ns - now) <= 0;
        }
        return false;
    }
};

// Alt modülleri re-export et
pub const scheduler = @import("scheduler.zig");
pub const watchdog = @import("watchdog.zig");

pub const Scheduler = scheduler.Scheduler;
pub const TaskWatchdog = watchdog.TaskWatchdog;
pub const WatchdogMetrics = watchdog.WatchdogMetrics;

test "task yasam dongusu completed'e kadar" {
    var t = Task.init(1, null, "task1", 1000, null, null);
    try t.transition(.queued);
    try t.transition(.running);
    try t.transition(.waiting);
    try t.transition(.running);
    try t.transition(.completed);
    try std.testing.expect(t.state == .completed);
    try std.testing.expect(t.state.isTerminal());
}

test "cancellation yolu waiting -> cancelling -> completed" {
    var t = Task.init(2, null, "task2", 1000, null, null);
    try t.transition(.queued);
    try t.transition(.running);
    try t.transition(.waiting);
    try t.transition(.cancelling);
    try t.transition(.completed);
    try std.testing.expect(t.state == .completed);
}

test "yield noktasinda cancellation garantisi" {
    var t = Task.init(3, null, "task3", 1000, null, null);
    try t.transition(.queued);
    try t.transition(.running);

    try t.pollYield(); // İptal yokken başarılı

    t.cancel_token.cancel();
    try std.testing.expectError(error.Cancelled, t.pollYield());
    try std.testing.expectEqual(TaskState.cancelling, t.state);
}

test "parent cancel token child task'a aktarilir" {
    var parent_token = CancellationToken.init();
    var child_task = Task.init(4, 1, "child", 1000, null, &parent_token);
    try child_task.transition(.queued);
    try child_task.transition(.running);

    parent_token.cancel();
    try std.testing.expectError(error.Cancelled, child_task.pollYield());
    try std.testing.expectEqual(TaskState.cancelling, child_task.state);
}

test "deadline asimi tespiti" {
    const FakeClock = struct {
        var now_val: i128 = 1_000_000;
        fn f() i128 {
            return now_val;
        }
    };
    const dl = Deadline.fromMillis(FakeClock.f, 50);
    var t = Task.init(5, null, "timed", FakeClock.now_val, dl, null);

    try std.testing.expect(!t.isDeadlineExceeded(FakeClock.now_val));
    FakeClock.now_val += 60 * std.time.ns_per_ms;
    try std.testing.expect(t.isDeadlineExceeded(FakeClock.now_val));
}
