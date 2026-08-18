//! Task Watchdog (tasarım Bölüm 3.2, 8, Doğrulama 12).
//!
//! - Deadline aşımı izleme (monotonic deadline)
//! - Yetim görev (orphan child) tespiti ve temizliği
//! - Görev sızıntısı (task leak) tespiti
//! - Kaynak ve sahiplik takibi + metrikler
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");
const task_mod = @import("task.zig");
const scheduler_mod = @import("scheduler.zig");

pub const Task = task_mod.Task;
pub const TaskState = task_mod.TaskState;
pub const Scheduler = scheduler_mod.Scheduler;

pub const WatchdogMetrics = struct {
    deadlines_exceeded: u64 = 0,
    orphans_detected: u64 = 0,
    leaks_detected: u64 = 0,
    total_checks: u64 = 0,
};

pub const TaskWatchdog = struct {
    metrics: WatchdogMetrics = .{},

    pub fn init() TaskWatchdog {
        return .{};
    }

    /// Süresi dolmuş görevleri tespit eder ve iptal eder (Doğrulama 12).
    pub fn checkDeadlines(self: *TaskWatchdog, scheduler: *Scheduler, now: i128) usize {
        var count: usize = 0;
        var it = scheduler.tasks.valueIterator();

        while (it.next()) |task_ptr| {
            const t = task_ptr.*;
            if (t.state.isTerminal()) continue;

            if (t.isDeadlineExceeded(now)) {
                t.requestCancel();
                _ = scheduler.finish(t.id, .{ .failed = "deadline_exceeded" }) catch {};
                self.metrics.deadlines_exceeded += 1;
                count += 1;
            }
        }

        return count;
    }

    /// Üst görevi (parent) tamamlanmış veya başarısız olmuş yetim görevleri (orphan child)
    /// tespit eder ve iptal eder (tasarım 3.2).
    pub fn detectOrphans(self: *TaskWatchdog, scheduler: *Scheduler) usize {
        var count: usize = 0;
        var it = scheduler.tasks.valueIterator();

        while (it.next()) |task_ptr| {
            const child = task_ptr.*;
            if (child.state.isTerminal()) continue;

            if (child.parent_id) |pid| {
                if (scheduler.getTask(pid)) |parent| {
                    if (parent.state.isTerminal()) {
                        // Üst görev bitmiş ama alt görev hala çalışıyor/bekliyor -> yetim
                        child.requestCancel();
                        _ = scheduler.finish(child.id, .{ .failed = "orphan_parent_finished" }) catch {};
                        self.metrics.orphans_detected += 1;
                        count += 1;
                    }
                } else {
                    // Üst görev sistemde hiç yok -> yetim
                    child.requestCancel();
                    _ = scheduler.finish(child.id, .{ .failed = "orphan_parent_missing" }) catch {};
                    self.metrics.orphans_detected += 1;
                    count += 1;
                }
            }
        }

        return count;
    }

    /// Maksimum yaş sınırını aşmış ve hala bitmemiş görevleri (leak) tespit eder.
    pub fn detectLeaks(self: *TaskWatchdog, scheduler: *Scheduler, max_age_ns: i128, now: i128) usize {
        var count: usize = 0;
        var it = scheduler.tasks.valueIterator();

        while (it.next()) |task_ptr| {
            const t = task_ptr.*;
            if (t.state.isTerminal()) continue;

            const age = now - t.created_at;
            if (age > max_age_ns) {
                t.requestCancel();
                _ = scheduler.finish(t.id, .{ .failed = "task_leaked_exceeded_max_age" }) catch {};
                self.metrics.leaks_detected += 1;
                count += 1;
            }
        }

        return count;
    }

    /// Tüm kontrolleri sırayla çalıştırır.
    pub fn runAllChecks(self: *TaskWatchdog, scheduler: *Scheduler, now: i128, max_age_ns: i128) void {
        self.metrics.total_checks += 1;
        _ = self.checkDeadlines(scheduler, now);
        _ = self.detectOrphans(scheduler);
        _ = self.detectLeaks(scheduler, max_age_ns, now);
    }
};

test "watchdog deadline asimini tespit eder ve sonlandirir" {
    var s = try Scheduler.init(std.testing.allocator, 4, 8);
    defer s.deinit();

    var dog = TaskWatchdog.init();

    const FakeClock = struct {
        var now_val: i128 = 1_000_000;
        fn f() i128 {
            return now_val;
        }
    };

    const dl = task_mod.Deadline.fromMillis(FakeClock.f, 50);
    var t = Task.init(1, null, "timed_task", FakeClock.now_val, dl, null);
    try s.submit(&t);
    _ = try s.nextReady(); // running

    // Saat henüz dolmadı
    try std.testing.expectEqual(@as(usize, 0), dog.checkDeadlines(&s, FakeClock.now_val));
    try std.testing.expectEqual(TaskState.running, t.state);

    // 60ms sonra süresi dolar
    FakeClock.now_val += 60 * std.time.ns_per_ms;
    const timed_out = dog.checkDeadlines(&s, FakeClock.now_val);
    try std.testing.expectEqual(@as(usize, 1), timed_out);
    try std.testing.expectEqual(TaskState.failed, t.state);
    try std.testing.expectEqualStrings("deadline_exceeded", t.failure_reason.?);
    try std.testing.expectEqual(@as(u64, 1), dog.metrics.deadlines_exceeded);
}

test "watchdog yetim (orphan) gorevleri temizler" {
    var s = try Scheduler.init(std.testing.allocator, 4, 8);
    defer s.deinit();

    var dog = TaskWatchdog.init();

    var parent = Task.init(10, null, "parent", 1000, null, null);
    var child = Task.init(11, 10, "child", 1000, null, null);

    try s.submit(&parent);
    try s.submit(&child);

    _ = try s.nextReady(); // parent running
    _ = try s.nextReady(); // child running

    // Parent tamamlanır
    try s.finish(10, .{ .completed = {} });
    try std.testing.expectEqual(TaskState.completed, parent.state);
    try std.testing.expectEqual(TaskState.running, child.state);

    // Watchdog yetim çocuğu yakalar
    const orphans = dog.detectOrphans(&s);
    try std.testing.expectEqual(@as(usize, 1), orphans);
    try std.testing.expectEqual(TaskState.failed, child.state);
    try std.testing.expectEqualStrings("orphan_parent_finished", child.failure_reason.?);
    try std.testing.expectEqual(@as(u64, 1), dog.metrics.orphans_detected);
}

test "watchdog leak tespiti" {
    var s = try Scheduler.init(std.testing.allocator, 4, 8);
    defer s.deinit();

    var dog = TaskWatchdog.init();

    const created_at: i128 = 1000;
    var t = Task.init(20, null, "stuck_task", created_at, null, null);
    try s.submit(&t);
    _ = try s.nextReady();

    const max_age: i128 = 5 * std.time.ns_per_s;
    const now_ok: i128 = created_at + 2 * std.time.ns_per_s;
    try std.testing.expectEqual(@as(usize, 0), dog.detectLeaks(&s, max_age, now_ok));

    const now_leaked: i128 = created_at + 10 * std.time.ns_per_s;
    const leaked = dog.detectLeaks(&s, max_age, now_leaked);
    try std.testing.expectEqual(@as(usize, 1), leaked);
    try std.testing.expectEqual(TaskState.failed, t.state);
    try std.testing.expectEqualStrings("task_leaked_exceeded_max_age", t.failure_reason.?);
}
