//! Bounded queue + backpressure (tasarım Bölüm 3.1, 8).
//!
//! Kuyruk kapasitesi sabittir: doluysa `push` error.QueueFull döner (backpressure),
//! boşsa `pop` error.QueueEmpty döner. Sınırsız event/transcript buffer tutulmaz
//! (Bölüm 8: kuyruklar bounded olur).

const std = @import("std");

pub const QueueError = error{
    QueueFull,
    QueueEmpty,
};

/// Sabit kapasiteli, FIFO, halka tampon kuyruk.
pub fn BoundedQueue(comptime T: type) type {
    return struct {
        const Self = @This();

        items: []T,
        head: usize = 0,
        len: usize = 0,
        allocator: std.mem.Allocator,

        pub fn init(allocator: std.mem.Allocator, cap: usize) !Self {
            if (cap == 0) return error.InvalidCapacity;
            return .{
                .items = try allocator.alloc(T, cap),
                .allocator = allocator,
            };
        }

        pub fn deinit(self: *Self) void {
            self.allocator.free(self.items);
            self.* = undefined;
        }

        pub fn capacity(self: *const Self) usize {
            return self.items.len;
        }

        pub fn length(self: *const Self) usize {
            return self.len;
        }

        pub fn isEmpty(self: *const Self) bool {
            return self.len == 0;
        }

        pub fn isFull(self: *const Self) bool {
            return self.len == self.items.len;
        }

        /// Doluysa error.QueueFull (backpressure).
        pub fn push(self: *Self, item: T) QueueError!void {
            if (self.isFull()) return error.QueueFull;
            const tail = (self.head + self.len) % self.items.len;
            self.items[tail] = item;
            self.len += 1;
        }

        /// Dolu değilse ekler ve true döner, doluysa false döner.
        pub fn tryPush(self: *Self, item: T) bool {
            self.push(item) catch return false;
            return true;
        }

        /// Boşsa error.QueueEmpty.
        pub fn pop(self: *Self) QueueError!T {
            if (self.isEmpty()) return error.QueueEmpty;
            const item = self.items[self.head];
            self.head = (self.head + 1) % self.items.len;
            self.len -= 1;
            return item;
        }

        /// Baştaki elemanı çıkarmadan inceler.
        pub fn peek(self: *const Self) ?*const T {
            if (self.isEmpty()) return null;
            return &self.items[self.head];
        }

        /// Kuyruğu sıfırlar.
        pub fn clear(self: *Self) void {
            self.head = 0;
            self.len = 0;
        }

        /// Kuyruktaki elemanları hedef dilime deterministik olarak boşaltır (drainage).
        /// Boşaltılan eleman sayısını döner.
        pub fn drainInto(self: *Self, dest: []T) usize {
            var count: usize = 0;
            while (count < dest.len and !self.isEmpty()) {
                dest[count] = self.pop() catch break;
                count += 1;
            }
            return count;
        }
    };
}

test "fifo sira korunur" {
    var q = try BoundedQueue(u32).init(std.testing.allocator, 4);
    defer q.deinit();
    try q.push(1);
    try q.push(2);
    try q.push(3);
    try std.testing.expectEqual(@as(u32, 1), try q.pop());
    try std.testing.expectEqual(@as(u32, 2), try q.pop());
    try std.testing.expectEqual(@as(u32, 3), try q.pop());
    try std.testing.expect(q.isEmpty());
}

test "dolu kuyruk backpressure uygular" {
    var q = try BoundedQueue(u8).init(std.testing.allocator, 2);
    defer q.deinit();
    try q.push(1);
    try q.push(2);
    try std.testing.expect(q.isFull());
    try std.testing.expectError(error.QueueFull, q.push(3));
    try std.testing.expect(!q.tryPush(4));
}

test "bos kuyruk pop reddeder" {
    var q = try BoundedQueue(u8).init(std.testing.allocator, 2);
    defer q.deinit();
    try std.testing.expectError(error.QueueEmpty, q.pop());
    try std.testing.expect(q.peek() == null);
}

test "peek elemani cikarmaz" {
    var q = try BoundedQueue(u32).init(std.testing.allocator, 2);
    defer q.deinit();
    try q.push(42);
    try std.testing.expectEqual(@as(u32, 42), q.peek().?.*);
    try std.testing.expectEqual(@as(usize, 1), q.length());
    try std.testing.expectEqual(@as(u32, 42), try q.pop());
}

test "drainInto deterministik bosaltma" {
    var q = try BoundedQueue(u32).init(std.testing.allocator, 5);
    defer q.deinit();
    try q.push(10);
    try q.push(20);
    try q.push(30);

    var dest: [5]u32 = undefined;
    const drained = q.drainInto(&dest);
    try std.testing.expectEqual(@as(usize, 3), drained);
    try std.testing.expectEqualSlices(u32, &[_]u32{ 10, 20, 30 }, dest[0..3]);
    try std.testing.expect(q.isEmpty());
}

test "halka tampon sarma (wrap)" {
    var q = try BoundedQueue(u32).init(std.testing.allocator, 3);
    defer q.deinit();
    try q.push(1);
    try q.push(2);
    try q.push(3);
    _ = try q.pop(); // head ileri
    try q.push(4); // tail sarmalı yazar
    try std.testing.expectEqual(@as(u32, 2), try q.pop());
    try std.testing.expectEqual(@as(u32, 3), try q.pop());
    try std.testing.expectEqual(@as(u32, 4), try q.pop());
    try std.testing.expect(q.isEmpty());
}
