//! Scheduler ve Bounded Task Admission (tasarım Bölüm 3.2, 8).
//!
//! - Bounded aktif görev kapasitesi (`max_active`)
//! - Bounded hazır kuyruğu (`ready_queue`) + backpressure
//! - Deterministik task planlama ve serbest bırakma
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");
const io = @import("../omnitrix-io/io.zig");
const task_mod = @import("task.zig");

pub const Task = task_mod.Task;
pub const TaskState = task_mod.TaskState;
pub const TaskOutcome = task_mod.TaskOutcome;

pub const SchedulerError = error{
    MaxActiveReached,
    QueueFull,
    TaskNotFound,
    InvalidTransition,
    InvalidCapacity,
};

pub const Scheduler = struct {
    allocator: std.mem.Allocator,
    max_active: usize,
    active_count: usize = 0,
    ready_queue: io.BoundedQueue(u64),
    tasks: std.AutoHashMap(u64, *Task),

    pub fn init(allocator: std.mem.Allocator, max_active: usize, queue_capacity: usize) !Scheduler {
        if (max_active == 0 or queue_capacity == 0) return error.InvalidCapacity;

        var q = try io.BoundedQueue(u64).init(allocator, queue_capacity);
        errdefer q.deinit();

        return .{
            .allocator = allocator,
            .max_active = max_active,
            .active_count = 0,
            .ready_queue = q,
            .tasks = std.AutoHashMap(u64, *Task).init(allocator),
        };
    }

    pub fn deinit(self: *Scheduler) void {
        self.ready_queue.deinit();
        self.tasks.deinit();
        self.* = undefined;
    }

    pub fn getTask(self: *const Scheduler, id: u64) ?*Task {
        return self.tasks.get(id);
    }

    pub fn activeCount(self: *const Scheduler) usize {
        return self.active_count;
    }

    pub fn queuedCount(self: *const Scheduler) usize {
        return self.ready_queue.length();
    }

    pub fn totalRegistered(self: *const Scheduler) usize {
        return self.tasks.count();
    }

    /// Yeni bir görevi kabul eder (admission) ve hazır kuyruğuna ekler.
    /// Kuyruk doluysa error.QueueFull döner (backpressure).
    pub fn submit(self: *Scheduler, task: *Task) SchedulerError!void {
        if (self.ready_queue.isFull()) return error.QueueFull;

        task.transition(.queued) catch return error.InvalidTransition;
        self.tasks.put(task.id, task) catch return error.QueueFull;
        self.ready_queue.push(task.id) catch return error.QueueFull;
    }

    /// Aktif kontenjan varsa sıradaki görevi `queued -> running` yapar ve döner.
    pub fn nextReady(self: *Scheduler) SchedulerError!?*Task {
        if (self.active_count >= self.max_active) return null;
        if (self.ready_queue.isEmpty()) return null;

        const id = self.ready_queue.pop() catch return null;
        const task = self.tasks.get(id) orelse return error.TaskNotFound;

        if (task.state == .cancelling) {
            // Zaten iptal edilmişse doğrudan terminal duruma geçebilir
            return task;
        }

        task.transition(.running) catch return error.InvalidTransition;
        self.active_count += 1;
        return task;
    }

    /// Görevi sonlandırır (completed veya failed) ve aktif kontenjanı serbest bırakır.
    pub fn finish(self: *Scheduler, id: u64, outcome: TaskOutcome) SchedulerError!void {
        const task = self.tasks.get(id) orelse return error.TaskNotFound;

        if (task.state == .queued or task.state == .created) {
            _ = task.transition(.cancelling) catch {};
        }

        const was_active = task.state == .running or task.state == .waiting or task.state == .cancelling;

        switch (outcome) {
            .completed => {
                task.transition(.completed) catch return error.InvalidTransition;
            },
            .failed => |reason| {
                task.failure_reason = reason;
                task.transition(.failed) catch return error.InvalidTransition;
            },
        }

        if (was_active and self.active_count > 0) {
            self.active_count -= 1;
        }
    }

    /// Görevin durumunu waiting yapar (IO beklerken aktif slot boşaltılabilir veya tutulabilir).
    pub fn waitTask(self: *Scheduler, id: u64) SchedulerError!void {
        const task = self.tasks.get(id) orelse return error.TaskNotFound;
        task.transition(.waiting) catch return error.InvalidTransition;
    }

    /// Beklemedeki görevi tekrar running yapar.
    pub fn resumeTask(self: *Scheduler, id: u64) SchedulerError!void {
        const task = self.tasks.get(id) orelse return error.TaskNotFound;
        task.transition(.running) catch return error.InvalidTransition;
    }

    /// Görevi iptal eder.
    pub fn cancelTask(self: *Scheduler, id: u64) SchedulerError!void {
        const task = self.tasks.get(id) orelse return error.TaskNotFound;
        task.requestCancel();
    }
};

test "scheduler submit, nextReady ve finish akisi" {
    var s = try Scheduler.init(std.testing.allocator, 2, 4);
    defer s.deinit();

    var t1 = Task.init(1, null, "task1", 100, null, null);
    var t2 = Task.init(2, null, "task2", 100, null, null);
    var t3 = Task.init(3, null, "task3", 100, null, null);

    try s.submit(&t1);
    try s.submit(&t2);
    try s.submit(&t3);

    try std.testing.expectEqual(@as(usize, 3), s.queuedCount());
    try std.testing.expectEqual(@as(usize, 0), s.activeCount());

    // 1. görevi al -> active 1
    const r1 = (try s.nextReady()).?;
    try std.testing.expectEqual(@as(u64, 1), r1.id);
    try std.testing.expectEqual(TaskState.running, r1.state);
    try std.testing.expectEqual(@as(usize, 1), s.activeCount());

    // 2. görevi al -> active 2 (max_active doldu)
    const r2 = (try s.nextReady()).?;
    try std.testing.expectEqual(@as(u64, 2), r2.id);
    try std.testing.expectEqual(@as(usize, 2), s.activeCount());

    // 3. görevi almaya çalış -> limit dolduğu için null dönmeli
    const r3 = try s.nextReady();
    try std.testing.expect(r3 == null);

    // 1. görevi tamamla -> aktif kontenjan açılır
    try s.finish(1, .{ .completed = {} });
    try std.testing.expectEqual(TaskState.completed, t1.state);
    try std.testing.expectEqual(@as(usize, 1), s.activeCount());

    // Şimdi 3. görev alınabilir
    const r3_ready = (try s.nextReady()).?;
    try std.testing.expectEqual(@as(u64, 3), r3_ready.id);
    try std.testing.expectEqual(@as(usize, 2), s.activeCount());

    // 2 ve 3'ü bitir
    try s.finish(2, .{ .failed = "error test" });
    try s.finish(3, .{ .completed = {} });
    try std.testing.expectEqual(@as(usize, 0), s.activeCount());
}

test "scheduler queue full backpressure" {
    var s = try Scheduler.init(std.testing.allocator, 2, 2);
    defer s.deinit();

    var t1 = Task.init(1, null, "t1", 100, null, null);
    var t2 = Task.init(2, null, "t2", 100, null, null);
    var t3 = Task.init(3, null, "t3", 100, null, null);

    try s.submit(&t1);
    try s.submit(&t2);
    try std.testing.expectError(error.QueueFull, s.submit(&t3));
}
