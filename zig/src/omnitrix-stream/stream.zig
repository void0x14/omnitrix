//! omnitrix-stream (tasarım Bölüm 3.4, Doğrulama 11).
//!
//! Provider stream'i, tool output'u, markdown parçası ve agent event'lerini tek
//! bounded stream modelinde birleştirir. Her event'in revision'ı vardır; her
//! event'te tüm transcript yeniden serialize edilmez (Bölüm 3.4, 8: kuyruklar
//! bounded olur, sınırsız event buffer tutulmaz).

const std = @import("std");
const io = @import("../omnitrix-io/io.zig");

pub const StreamError = io.QueueError;

pub const EventKind = enum {
    provider_chunk,
    tool_output,
    markdown_part,
    agent_event,

    pub fn label(self: EventKind) []const u8 {
        return @tagName(self);
    }
};

/// Tek bir akış olayı.
pub const StreamEvent = struct {
    revision: u64 = 0,
    kind: EventKind,
    data: []const u8 = "",
    timestamp_ns: i128 = 0,
    task_id: ?u64 = null,
    session_id: ?[]const u8 = null,
    is_final: bool = false,
};

/// Bounded akış yöneticisi.
/// Kapasite sabittir; doluysa `push` error.QueueFull döner (backpressure).
/// Revision her başarılı push işleminde kesin artar.
pub const UnifiedStream = struct {
    queue: io.BoundedQueue(StreamEvent),
    revision: u64 = 0,
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator, cap: usize) !UnifiedStream {
        return .{
            .queue = try io.BoundedQueue(StreamEvent).init(allocator, cap),
            .revision = 0,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *UnifiedStream) void {
        self.queue.deinit();
        self.* = undefined;
    }

    pub fn capacity(self: *const UnifiedStream) usize {
        return self.queue.capacity();
    }

    pub fn length(self: *const UnifiedStream) usize {
        return self.queue.length();
    }

    pub fn isEmpty(self: *const UnifiedStream) bool {
        return self.queue.isEmpty();
    }

    pub fn isFull(self: *const UnifiedStream) bool {
        return self.queue.isFull();
    }

    pub fn currentRevision(self: *const UnifiedStream) u64 {
        return self.revision;
    }

    /// Yeni bir event ekler. Bounded kapasite aşıldığında error.QueueFull döner (backpressure).
    /// Başarılı eklemede revision 1 artırılır ve event'e atanır.
    pub fn push(
        self: *UnifiedStream,
        kind: EventKind,
        data: []const u8,
        timestamp_ns: i128,
        task_id: ?u64,
        session_id: ?[]const u8,
        is_final: bool,
    ) StreamError!u64 {
        if (self.queue.isFull()) return error.QueueFull;

        const next_rev = self.revision + 1;
        const ev = StreamEvent{
            .revision = next_rev,
            .kind = kind,
            .data = data,
            .timestamp_ns = timestamp_ns,
            .task_id = task_id,
            .session_id = session_id,
            .is_final = is_final,
        };

        try self.queue.push(ev);
        self.revision = next_rev;
        return next_rev;
    }

    /// En eski olayı çıkarır ve döner.
    pub fn pop(self: *UnifiedStream) StreamError!StreamEvent {
        return self.queue.pop();
    }

    /// En eski olayı çıkarmadan inceler.
    pub fn peek(self: *const UnifiedStream) ?*const StreamEvent {
        return self.queue.peek();
    }

    /// Belirli bir revision'dan sonraki olayları `out` tamponuna kopyalar (safe-tail reader, Doğrulama 11).
    /// Tüm transcript'i yeniden serialize etmeden sadece yeni gelen parçaları okur.
    pub fn readSince(self: *const UnifiedStream, since_rev: u64, out: []StreamEvent) usize {
        var count: usize = 0;
        const len = self.queue.length();

        var i: usize = 0;
        while (i < len and count < out.len) : (i += 1) {
            const idx = (self.queue.head + i) % self.queue.items.len;
            const ev = self.queue.items[idx];
            if (ev.revision > since_rev) {
                out[count] = ev;
                count += 1;
            }
        }

        return count;
    }
};

/// Generic Stream alias (geriye dönük uyumluluk için).
pub fn Stream(comptime T: type) type {
    return struct {
        const Self = @This();

        queue: io.BoundedQueue(T),
        revision: u64 = 0,
        allocator: std.mem.Allocator,

        pub fn init(allocator: std.mem.Allocator, cap: usize) !Self {
            return .{
                .queue = try io.BoundedQueue(T).init(allocator, cap),
                .allocator = allocator,
            };
        }

        pub fn deinit(self: *Self) void {
            self.queue.deinit();
            self.* = undefined;
        }

        pub fn capacity(self: *const Self) usize {
            return self.queue.capacity();
        }

        pub fn length(self: *const Self) usize {
            return self.queue.length();
        }

        pub fn push(self: *Self, item: T) StreamError!void {
            try self.queue.push(item);
            self.revision += 1;
        }

        pub fn pop(self: *Self) StreamError!T {
            return self.queue.pop();
        }

        pub fn currentRevision(self: *const Self) u64 {
            return self.revision;
        }
    };
}

test "unified stream event ekleme ve revision artisi" {
    var s = try UnifiedStream.init(std.testing.allocator, 4);
    defer s.deinit();

    try std.testing.expectEqual(@as(u64, 0), s.currentRevision());

    const r1 = try s.push(.provider_chunk, "tok1", 100, 1, "sess1", false);
    try std.testing.expectEqual(@as(u64, 1), r1);
    try std.testing.expectEqual(@as(u64, 1), s.currentRevision());

    const r2 = try s.push(.tool_output, "out1", 200, 1, "sess1", false);
    try std.testing.expectEqual(@as(u64, 2), r2);
    try std.testing.expectEqual(@as(u64, 2), s.currentRevision());

    const ev1 = try s.pop();
    try std.testing.expectEqual(EventKind.provider_chunk, ev1.kind);
    try std.testing.expectEqualStrings("tok1", ev1.data);
    try std.testing.expectEqual(@as(u64, 1), ev1.revision);
}

test "unified stream bounded backpressure" {
    var s = try UnifiedStream.init(std.testing.allocator, 2);
    defer s.deinit();

    _ = try s.push(.provider_chunk, "p1", 100, null, null, false);
    _ = try s.push(.provider_chunk, "p2", 200, null, null, false);
    try std.testing.expect(s.isFull());

    // 3. eleman backpressure vermelidir
    try std.testing.expectError(error.QueueFull, s.push(.provider_chunk, "p3", 300, null, null, false));

    _ = try s.pop();
    // Alan açıldıktan sonra eklenebilir
    const r3 = try s.push(.markdown_part, "# Header", 400, null, null, false);
    try std.testing.expectEqual(@as(u64, 3), r3);
}

test "unified stream safe-tail readSince (Dogrulama 11)" {
    var s = try UnifiedStream.init(std.testing.allocator, 8);
    defer s.deinit();

    _ = try s.push(.provider_chunk, "chunk1", 100, 1, null, false);
    _ = try s.push(.provider_chunk, "chunk2", 200, 1, null, false);
    _ = try s.push(.tool_output, "tool_res", 300, 1, null, false);
    _ = try s.push(.agent_event, "turn_end", 400, 1, null, true);

    var tail_buf: [4]StreamEvent = undefined;

    // Revision 2'den sonrakileri oku -> rev 3 (tool_output) ve rev 4 (agent_event) gelmeli
    const count = s.readSince(2, &tail_buf);
    try std.testing.expectEqual(@as(usize, 2), count);
    try std.testing.expectEqual(EventKind.tool_output, tail_buf[0].kind);
    try std.testing.expectEqualStrings("tool_res", tail_buf[0].data);
    try std.testing.expectEqual(EventKind.agent_event, tail_buf[1].kind);
    try std.testing.expectEqualStrings("turn_end", tail_buf[1].data);
    try std.testing.expect(tail_buf[1].is_final);
}
