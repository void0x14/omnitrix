//! StateStore — Tek-Süreç Authoritative Durum Merkezi (tasarım Bölüm 4, 3.2, 3.4, 6).
//!
//! Omnitrix tek bir ana süreçtir. StateStore, tüm alt sistemleri (AgentRuntime,
//! Scheduler, Stream, Ledger, Watchdog) doğrudan typed çağrılarla koordine eder.
//! Modüller arasında worker süreci, JSON/Protobuf IPC veya token başına serialization
//! YOKTUR (tasarım Bölüm 1, 4).
//!
//! TUI veya dış gözlemciler için hafif ve tutarlı `StateSnapshot` üretir.

const std = @import("std");
const io = @import("../omnitrix-io/io.zig");
const task_mod = @import("../omnitrix-task/task.zig");
const stream_mod = @import("../omnitrix-stream/stream.zig");
const ledger_mod = @import("../omnitrix-ledger/ledger.zig");
const agent_mod = @import("agent.zig");

pub const AgentStatus = agent_mod.AgentStatus;
pub const AgentStateMachine = agent_mod.AgentStateMachine;
pub const Task = task_mod.Task;
pub const TaskState = task_mod.TaskState;
pub const Scheduler = task_mod.Scheduler;
pub const TaskWatchdog = task_mod.TaskWatchdog;
pub const UnifiedStream = stream_mod.UnifiedStream;
pub const StreamEvent = stream_mod.StreamEvent;
pub const EventKind = stream_mod.EventKind;
pub const Ledger = ledger_mod.Ledger;
pub const MutationRecord = ledger_mod.MutationRecord;
pub const MutationKind = ledger_mod.MutationKind;
pub const Actor = ledger_mod.Actor;
pub const Hash = ledger_mod.Hash;

pub const StateSnapshot = struct {
    agent_status: AgentStatus,
    current_task_id: ?u64,
    active_tool: ?[]const u8,
    active_tasks: usize,
    queued_tasks: usize,
    stream_revision: u64,
    ledger_entry_count: usize,
    turn_id: u64,
    session_id: []const u8,
};

pub const RuntimeError = error{
    TaskCreationFailed,
    SchedulerAdmissionFailed,
    InvalidTransition,
    StreamError,
    LedgerError,
    RuntimeShuttingDown,
};

pub const StateStore = struct {
    allocator: std.mem.Allocator,
    session_id: []const u8,
    turn_id: u64 = 0,
    next_task_id: u64 = 1,
    is_shutting_down: bool = false,

    agent: AgentStateMachine,
    scheduler: Scheduler,
    watchdog: TaskWatchdog,
    stream: UnifiedStream,
    ledger: Ledger,

    // Task nesnelerinin yaşam döngüsünü yöneten liste
    allocated_tasks: std.ArrayList(*Task),

    pub fn init(
        allocator: std.mem.Allocator,
        session_id: []const u8,
        max_active_tasks: usize,
        task_queue_cap: usize,
        stream_cap: usize,
    ) !StateStore {
        var sched = try Scheduler.init(allocator, max_active_tasks, task_queue_cap);
        errdefer sched.deinit();

        var strm = try UnifiedStream.init(allocator, stream_cap);
        errdefer strm.deinit();

        const ledg = Ledger.init(allocator);

        return .{
            .allocator = allocator,
            .session_id = try allocator.dupe(u8, session_id),
            .turn_id = 0,
            .next_task_id = 1,
            .is_shutting_down = false,
            .agent = AgentStateMachine.init(),
            .scheduler = sched,
            .watchdog = TaskWatchdog.init(),
            .stream = strm,
            .ledger = ledg,
            .allocated_tasks = std.ArrayList(*Task).empty,
        };
    }

    pub fn deinit(self: *StateStore) void {
        self.shutdown();
        for (self.allocated_tasks.items) |t| {
            self.allocator.destroy(t);
        }
        self.allocated_tasks.deinit(self.allocator);
        self.ledger.deinit();
        self.stream.deinit();
        self.scheduler.deinit();
        self.allocator.free(self.session_id);
        self.* = undefined;
    }

    /// TUI ve izleyiciler için anlık immutable snapshot oluşturur.
    pub fn createSnapshot(self: *const StateStore) StateSnapshot {
        return .{
            .agent_status = self.agent.status,
            .current_task_id = self.agent.current_task_id,
            .active_tool = self.agent.active_tool_name,
            .active_tasks = self.scheduler.activeCount(),
            .queued_tasks = self.scheduler.queuedCount(),
            .stream_revision = self.stream.currentRevision(),
            .ledger_entry_count = self.ledger.entryCount(),
            .turn_id = self.turn_id,
            .session_id = self.session_id,
        };
    }

    /// Yeni bir kullanıcı turu başlatır, görevi planlar ve agent'ı thinking durumuna geçirir.
    pub fn startTurn(self: *StateStore, name: []const u8, timeout_ms: ?i128) RuntimeError!u64 {
        if (self.is_shutting_down) return error.RuntimeShuttingDown;

        self.turn_id += 1;
        const task_id = self.next_task_id;
        self.next_task_id += 1;

        const now = io.Monotonic.now();
        const deadline: ?io.Deadline = if (timeout_ms) |ms|
            io.Deadline.fromMillis(io.Monotonic.now, ms)
        else
            null;

        const task_ptr = self.allocator.create(Task) catch return error.TaskCreationFailed;
        task_ptr.* = Task.init(task_id, null, name, now, deadline, null);
        self.allocated_tasks.append(self.allocator, task_ptr) catch {
            self.allocator.destroy(task_ptr);
            return error.TaskCreationFailed;
        };

        self.scheduler.submit(task_ptr) catch return error.SchedulerAdmissionFailed;
        self.agent.startThinking(task_id) catch return error.InvalidTransition;

        _ = self.stream.push(.agent_event, "turn_started", now, task_id, self.session_id, false) catch {};

        return task_id;
    }

    /// Planlanan sıradaki görevi çalıştırır.
    pub fn stepScheduler(self: *StateStore) RuntimeError!?*Task {
        if (self.is_shutting_down) return null;
        return self.scheduler.nextReady() catch return error.SchedulerAdmissionFailed;
    }

    /// Provider'dan gelen token/chunk verisini bounded stream'e aktarır.
    pub fn streamProviderChunk(self: *StateStore, chunk: []const u8, is_final: bool) RuntimeError!u64 {
        if (self.is_shutting_down) return error.RuntimeShuttingDown;

        if (self.agent.status != .streaming_response) {
            self.agent.startStreaming() catch return error.InvalidTransition;
        }

        const now = io.Monotonic.now();
        const rev = self.stream.push(
            .provider_chunk,
            chunk,
            now,
            self.agent.current_task_id,
            self.session_id,
            is_final,
        ) catch return error.StreamError;

        if (is_final) {
            self.agent.awaitInput() catch {};
        }

        return rev;
    }

    /// Tool çalıştırma başlangıcını kaydeder.
    pub fn startTool(self: *StateStore, tool_name: []const u8) RuntimeError!void {
        if (self.is_shutting_down) return error.RuntimeShuttingDown;
        self.agent.startToolExecution(tool_name) catch return error.InvalidTransition;
    }

    /// Tool çıktısını bounded stream'e aktarır ve agent durumunu günceller.
    pub fn finishTool(self: *StateStore, output: []const u8) RuntimeError!u64 {
        if (self.is_shutting_down) return error.RuntimeShuttingDown;

        const now = io.Monotonic.now();
        const rev = self.stream.push(
            .tool_output,
            output,
            now,
            self.agent.current_task_id,
            self.session_id,
            false,
        ) catch return error.StreamError;

        // Tool bitince tekrar thinking durumuna geç
        if (self.agent.current_task_id) |tid| {
            self.agent.startThinking(tid) catch {};
        }

        return rev;
    }

    /// FileMutationLedger'a dosya değişikliği kaydeder.
    pub fn recordFileMutation(
        self: *StateStore,
        path: []const u8,
        kind: MutationKind,
        actor: Actor,
        after_hash: ?Hash,
        before_hash: ?Hash,
    ) RuntimeError!*MutationRecord {
        const now = io.Monotonic.now();
        return self.ledger.record(path, kind, actor, now, after_hash, before_hash) catch return error.LedgerError;
    }

    /// Görevi başarıyla veya hatayla tamamlar.
    pub fn finishTask(self: *StateStore, task_id: u64, outcome: task_mod.TaskOutcome) void {
        self.scheduler.finish(task_id, outcome) catch {};
        const now = io.Monotonic.now();

        switch (outcome) {
            .completed => {
                _ = self.stream.push(.agent_event, "task_completed", now, task_id, self.session_id, true) catch {};
                self.agent.setIdle() catch {};
            },
            .failed => |err| {
                _ = self.stream.push(.agent_event, err, now, task_id, self.session_id, true) catch {};
                self.agent.fail(err);
            },
        }
    }

    /// Yeni bir alt görev (child task) başlatır ve ebeveynin iptal belirtecini (cancel token) bağlar.
    pub fn spawnChildTask(
        self: *StateStore,
        parent_id: u64,
        name: []const u8,
        timeout_ms: ?i128,
    ) RuntimeError!u64 {
        if (self.is_shutting_down) return error.RuntimeShuttingDown;

        const parent = self.scheduler.getTask(parent_id) orelse return error.TaskCreationFailed;

        const task_id = self.next_task_id;
        self.next_task_id += 1;

        const now = io.Monotonic.now();
        const deadline: ?io.Deadline = if (timeout_ms) |ms|
            io.Deadline.fromMillis(io.Monotonic.now, ms)
        else
            null;

        const task_ptr = self.allocator.create(Task) catch return error.TaskCreationFailed;
        task_ptr.* = Task.init(task_id, parent_id, name, now, deadline, &parent.cancel_token);
        self.allocated_tasks.append(self.allocator, task_ptr) catch {
            self.allocator.destroy(task_ptr);
            return error.TaskCreationFailed;
        };

        self.scheduler.submit(task_ptr) catch return error.SchedulerAdmissionFailed;
        return task_id;
    }

    /// Görevi iptal eder.
    pub fn cancelTask(self: *StateStore, task_id: u64) void {
        self.scheduler.cancelTask(task_id) catch {};
    }

    /// Sonlanmış (completed/failed) görevleri temizleyip belleklerini iade eder.
    pub fn reapTerminalTasks(self: *StateStore) usize {
        var reaped: usize = 0;
        var i: usize = 0;
        while (i < self.allocated_tasks.items.len) {
            const t = self.allocated_tasks.items[i];
            if (t.state.isTerminal()) {
                _ = self.scheduler.tasks.remove(t.id);
                self.allocator.destroy(t);
                _ = self.allocated_tasks.swapRemove(i);
                reaped += 1;
            } else {
                i += 1;
            }
        }
        return reaped;
    }

    /// Watchdog kontrollerini çalıştırır.
    pub fn runWatchdog(self: *StateStore, max_age_ns: i128) void {
        const now = io.Monotonic.now();
        self.watchdog.runAllChecks(&self.scheduler, now, max_age_ns);
    }

    /// Runtime'ı deterministik olarak sonlandırır.
    pub fn shutdown(self: *StateStore) void {
        if (self.is_shutting_down) return;
        self.is_shutting_down = true;

        // Aktif görevleri iptal et
        var it = self.scheduler.tasks.valueIterator();
        while (it.next()) |task_ptr| {
            if (!task_ptr.*.state.isTerminal()) {
                task_ptr.*.requestCancel();
            }
        }

        self.agent.terminate();
    }
};

test "state store tek-surec koordinasyonu ve snapshot" {
    var store = try StateStore.init(std.testing.allocator, "sess_123", 2, 4, 16);
    defer store.deinit();

    // Başlangıç snapshot'ı
    var snap = store.createSnapshot();
    try std.testing.expectEqual(AgentStatus.idle, snap.agent_status);
    try std.testing.expectEqual(@as(usize, 0), snap.active_tasks);
    try std.testing.expectEqual(@as(u64, 0), snap.stream_revision);

    // Turn başlat -> task kuyruğa girer, agent thinking olur
    const task_id = try store.startTurn("analyze_code", 1000);
    try std.testing.expectEqual(@as(u64, 1), task_id);

    snap = store.createSnapshot();
    try std.testing.expectEqual(AgentStatus.thinking, snap.agent_status);
    try std.testing.expectEqual(@as(usize, 1), snap.queued_tasks);

    // Scheduler'dan görevi al
    const running_task = (try store.stepScheduler()).?;
    try std.testing.expectEqual(@as(u64, 1), running_task.id);
    try std.testing.expectEqual(TaskState.running, running_task.state);

    // Tool çalıştır
    try store.startTool("file_read");
    snap = store.createSnapshot();
    try std.testing.expectEqual(AgentStatus.executing_tool, snap.agent_status);
    try std.testing.expectEqualStrings("file_read", snap.active_tool.?);

    const tool_rev = try store.finishTool("file content here");
    try std.testing.expect(tool_rev > 0);

    // Stream provider chunk
    const chunk_rev = try store.streamProviderChunk("Hello from model", false);
    try std.testing.expect(chunk_rev > tool_rev);

    snap = store.createSnapshot();
    try std.testing.expectEqual(AgentStatus.streaming_response, snap.agent_status);

    // File mutation kaydı
    const rec = try store.recordFileMutation("src/main.zig", .modified, .agent, null, null);
    try std.testing.expectEqualStrings("src/main.zig", rec.path);
    try std.testing.expectEqual(@as(usize, 1), store.ledger.entryCount());

    // Görevi bitir
    store.finishTask(task_id, .{ .completed = {} });
    snap = store.createSnapshot();
    try std.testing.expectEqual(AgentStatus.idle, snap.agent_status);
    try std.testing.expectEqual(@as(usize, 0), snap.active_tasks);
}

test "state store shutdown deterministik sonlandirma" {
    var store = try StateStore.init(std.testing.allocator, "sess_term", 2, 4, 16);
    defer store.deinit();

    const tid = try store.startTurn("task_to_cancel", 5000);
    _ = try store.stepScheduler();

    store.shutdown();
    const snap = store.createSnapshot();
    try std.testing.expectEqual(AgentStatus.terminated, snap.agent_status);

    const t = store.scheduler.getTask(tid).?;
    try std.testing.expect(t.cancel_token.isCancelled());
}

test "state store alt gorev olusturma ve terminal task temizligi" {
    var store = try StateStore.init(std.testing.allocator, "sess_child", 4, 8, 16);
    defer store.deinit();

    const parent_id = try store.startTurn("parent_task", 10000);
    const child_id = try store.spawnChildTask(parent_id, "child_subtask", 5000);

    const parent_task = store.scheduler.getTask(parent_id).?;
    const child_task = store.scheduler.getTask(child_id).?;

    try std.testing.expectEqual(@as(?u64, parent_id), child_task.parent_id);

    // Parent iptal edilince child cancel token'ı da iptal olmalı
    store.cancelTask(parent_id);
    try std.testing.expect(parent_task.cancel_token.isCancelled());
    try std.testing.expect(child_task.cancel_token.isCancelled());

    // Task'ları bitir ve reap yap
    store.finishTask(parent_id, .{ .failed = "cancelled" });
    store.finishTask(child_id, .{ .failed = "cancelled" });

    const reaped = store.reapTerminalTasks();
    try std.testing.expectEqual(@as(usize, 2), reaped);
    try std.testing.expectEqual(@as(usize, 0), store.allocated_tasks.items.len);
}
