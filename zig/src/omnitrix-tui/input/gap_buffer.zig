const std = @import("std");

/// A GapBuffer is an efficient text editing data structure.
/// Text before the cursor is stored before the gap, text after is stored after.
/// Insertions/deletions at the cursor are O(1) amortized.
pub const GapBuffer = struct {
    allocator: std.mem.Allocator,
    buf: []u8,
    gap_start: usize,
    gap_end: usize,
    len: usize,

    pub const empty_gap: usize = 0;

    pub fn init(allocator: std.mem.Allocator, initial_capacity: usize) !GapBuffer {
        const cap = if (initial_capacity < 64) 64 else initial_capacity;
        const buf = try allocator.alloc(u8, cap);
        return .{
            .allocator = allocator,
            .buf = buf,
            .gap_start = 0,
            .gap_end = cap,
            .len = 0,
        };
    }

    pub fn deinit(self: *GapBuffer) void {
        self.allocator.free(self.buf);
    }

    pub fn cursor(self: GapBuffer) usize {
        return self.gap_start;
    }

    pub fn length(self: GapBuffer) usize {
        return self.len;
    }

    pub fn isEmpty(self: GapBuffer) bool {
        return self.len == 0;
    }

    fn gapSize(self: GapBuffer) usize {
        return self.gap_end - self.gap_start;
    }

    fn ensureCapacity(self: *GapBuffer, needed: usize) !void {
        if (self.gapSize() >= needed) return;
        const new_cap = (self.buf.len * 2) + needed;
        const new_buf = try self.allocator.alloc(u8, new_cap);
        // Copy before-gap
        @memcpy(new_buf[0..self.gap_start], self.buf[0..self.gap_start]);
        // Copy after-gap
        const after_start = self.gap_end;
        const after_len = self.len - self.gap_start;
        const new_gap_end = new_cap - after_len;
        @memcpy(new_buf[new_gap_end..][0..after_len], self.buf[after_start..][0..after_len]);
        self.allocator.free(self.buf);
        self.buf = new_buf;
        self.gap_end = new_gap_end;
    }

    /// Move cursor to absolute position
    pub fn moveTo(self: *GapBuffer, pos: usize) void {
        const p = @min(pos, self.len);
        if (p < self.gap_start) {
            // Move gap left
            const move = self.gap_start - p;
            const new_gap_start = p;
            const new_gap_end = self.gap_end - move;
            std.mem.copyBackwards(u8, self.buf[new_gap_end..][0..move], self.buf[new_gap_start..][0..move]);
            self.gap_start = new_gap_start;
            self.gap_end = new_gap_end;
        } else if (p > self.gap_start) {
            // Move gap right
            const move = p - self.gap_start;
            const new_gap_start = p;
            const new_gap_end = self.gap_end + move;
            std.mem.copyForwards(u8, self.buf[self.gap_start..][0..move], self.buf[self.gap_end..][0..move]);
            self.gap_start = new_gap_start;
            self.gap_end = new_gap_end;
        }
    }

    /// Insert a single byte at cursor
    pub fn insertByte(self: *GapBuffer, byte: u8) !void {
        try self.ensureCapacity(1);
        self.buf[self.gap_start] = byte;
        self.gap_start += 1;
        self.len += 1;
    }

    /// Insert a unicode codepoint at cursor
    pub fn insertCodepoint(self: *GapBuffer, cp: u21) !void {
        var tmp: [4]u8 = undefined;
        const len = std.unicode.wtf8Encode(cp, &tmp) catch 1;
        try self.ensureCapacity(@as(usize, len));
        for (tmp[0..len]) |b| {
            self.buf[self.gap_start] = b;
            self.gap_start += 1;
        }
        self.len += len;
    }

    /// Insert text at cursor
    pub fn insertText(self: *GapBuffer, text: []const u8) !void {
        try self.ensureCapacity(text.len);
        for (text) |b| {
            self.buf[self.gap_start] = b;
            self.gap_start += 1;
        }
        self.len += text.len;
    }

    /// Delete one UTF-8 codepoint before the cursor.
    pub fn deleteBackward(self: *GapBuffer) ?u8 {
        if (self.gap_start == 0) return null;
        const start = self.previousBoundary(self.gap_start);
        const deleted = self.charAt(start);
        self.deleteRange(start, self.gap_start);
        return deleted;
    }

    /// Delete one UTF-8 codepoint after the cursor.
    pub fn deleteForward(self: *GapBuffer) ?u8 {
        if (self.gap_start >= self.len) return null;
        const end = self.nextBoundary(self.gap_start);
        const deleted = self.charAt(self.gap_start);
        self.deleteRange(self.gap_start, end);
        return deleted;
    }

    /// Delete range [start, end) from the buffer
    pub fn deleteRange(self: *GapBuffer, start: usize, end: usize) void {
        if (start >= end or end > self.len) return;
        self.moveTo(start);
        const delete_count = end - start;
        self.gap_end += delete_count;
        self.len -= delete_count;
    }

    /// Get character at position (relative to buffer start, not gap)
    pub fn charAt(self: GapBuffer, pos: usize) ?u8 {
        if (pos >= self.len) return null;
        if (pos < self.gap_start) {
            return self.buf[pos];
        } else {
            return self.buf[self.gap_end + (pos - self.gap_start)];
        }
    }

    /// Copy as much complete UTF-8 text as fits in `out`.
    pub fn getTextBounded(self: GapBuffer, out: []u8) []u8 {
        const wanted = @min(self.len, out.len);
        const before_len = @min(self.gap_start, wanted);
        const after_len = wanted - before_len;
        @memcpy(out[0..before_len], self.buf[0..before_len]);
        if (after_len > 0) {
            @memcpy(out[before_len..wanted], self.buf[self.gap_end..][0..after_len]);
        }
        var end = wanted;
        while (end > 0 and !std.unicode.utf8ValidateSlice(out[0..end])) end -= 1;
        return out[0..end];
    }

    /// Get all text as a contiguous slice. Callers must provide enough space.
    pub fn getText(self: GapBuffer, out: []u8) []u8 {
        if (out.len < self.len) return self.getTextBounded(out);
        return self.getTextBounded(out);
    }

    /// Get text as owned slice (caller must free)
    pub fn getTextOwned(self: GapBuffer) ![]u8 {
        const out = try self.allocator.alloc(u8, self.len);
        _ = self.getText(out);
        return out;
    }

    /// Clear all text
    pub fn clear(self: *GapBuffer) void {
        self.gap_start = 0;
        self.gap_end = self.buf.len;
        self.len = 0;
    }

    /// Set text (clear + insert)
    pub fn setText(self: *GapBuffer, text: []const u8) !void {
        self.clear();
        try self.insertText(text);
    }

    fn previousBoundary(self: GapBuffer, pos: usize) usize {
        var p = pos;
        while (p > 0) {
            p -= 1;
            const byte = self.charAt(p) orelse break;
            if ((byte & 0xc0) != 0x80) return p;
        }
        return 0;
    }

    fn nextBoundary(self: GapBuffer, pos: usize) usize {
        if (pos >= self.len) return self.len;
        const first = self.charAt(pos) orelse return pos;
        const sequence_len = std.unicode.utf8ByteSequenceLength(first) catch 1;
        return @min(self.len, pos + @as(usize, sequence_len));
    }

    fn codepointAt(self: GapBuffer, pos: usize) ?struct { cp: u21, len: usize } {
        if (pos >= self.len) return null;
        const first = self.charAt(pos) orelse return null;
        const sequence_len = std.unicode.utf8ByteSequenceLength(first) catch 1;
        const len = @min(@as(usize, sequence_len), self.len - pos);
        var bytes: [4]u8 = undefined;
        for (0..len) |i| bytes[i] = self.charAt(pos + i) orelse return null;
        const cp = std.unicode.wtf8Decode(bytes[0..len]) catch return .{ .cp = first, .len = 1 };
        return .{ .cp = cp, .len = len };
    }

    fn isSeparator(self: GapBuffer, pos: usize) bool {
        const item = self.codepointAt(pos) orelse return true;
        return item.cp == ' ' or item.cp == '\n' or item.cp == '\t' or item.cp == '\r';
    }

    /// Move cursor left by one UTF-8 codepoint.
    pub fn moveLeft(self: *GapBuffer) void {
        self.moveTo(self.previousBoundary(self.gap_start));
    }

    /// Move cursor right by one UTF-8 codepoint.
    pub fn moveRight(self: *GapBuffer) void {
        self.moveTo(self.nextBoundary(self.gap_start));
    }

    pub fn moveToLineStart(self: *GapBuffer) void {
        var pos = self.gap_start;
        while (pos > 0) {
            const previous = self.previousBoundary(pos);
            if ((self.codepointAt(previous) orelse break).cp == '\n') break;
            pos = previous;
        }
        self.moveTo(pos);
    }

    pub fn moveToLineEnd(self: *GapBuffer) void {
        var pos = self.gap_start;
        while (pos < self.len) {
            const item = self.codepointAt(pos) orelse break;
            if (item.cp == '\n') break;
            pos += item.len;
        }
        self.moveTo(pos);
    }

    pub fn moveToBeginning(self: *GapBuffer) void {
        self.moveTo(0);
    }

    pub fn moveToEnd(self: *GapBuffer) void {
        self.moveTo(self.len);
    }

    pub fn deleteWordBackward(self: *GapBuffer) void {
        const old_pos = self.gap_start;
        var pos = old_pos;
        while (pos > 0 and self.isSeparator(self.previousBoundary(pos))) pos = self.previousBoundary(pos);
        while (pos > 0 and !self.isSeparator(self.previousBoundary(pos))) pos = self.previousBoundary(pos);
        self.deleteRange(pos, old_pos);
    }

    pub fn deleteWordForward(self: *GapBuffer) void {
        var end = self.gap_start;
        while (end < self.len and !self.isSeparator(end)) end = self.nextBoundary(end);
        while (end < self.len and self.isSeparator(end)) end = self.nextBoundary(end);
        self.deleteRange(self.gap_start, end);
    }
};

test "GapBuffer insert and retrieve" {
    var gb = try GapBuffer.init(std.testing.allocator, 16);
    defer gb.deinit();

    try gb.insertText("Hello");
    try std.testing.expectEqual(@as(usize, 5), gb.length());
    try std.testing.expectEqual(@as(u8, 'H'), gb.charAt(0) orelse 0);
    try std.testing.expectEqual(@as(u8, 'o'), gb.charAt(4) orelse 0);

    try gb.insertText(" World");
    try std.testing.expectEqual(@as(usize, 11), gb.length());

    var out: [32]u8 = undefined;
    const text = gb.getText(&out);
    try std.testing.expectEqualStrings("Hello World", text);
}

test "GapBuffer delete backward" {
    var gb = try GapBuffer.init(std.testing.allocator, 16);
    defer gb.deinit();

    try gb.insertText("ABC");
    const deleted = gb.deleteBackward();
    try std.testing.expectEqual(@as(?u8, 'C'), deleted);
    try std.testing.expectEqual(@as(usize, 2), gb.length());

    var out: [32]u8 = undefined;
    const text = gb.getText(&out);
    try std.testing.expectEqualStrings("AB", text);
}

test "GapBuffer cursor movement" {
    var gb = try GapBuffer.init(std.testing.allocator, 16);
    defer gb.deinit();

    try gb.insertText("Hello");
    gb.moveTo(2);
    try gb.insertText("XX");
    try std.testing.expectEqual(@as(usize, 7), gb.length());

    var out: [32]u8 = undefined;
    const text = gb.getText(&out);
    try std.testing.expectEqualStrings("HeXXllo", text);
}

test "GapBuffer clear and set text" {
    var gb = try GapBuffer.init(std.testing.allocator, 16);
    defer gb.deinit();

    try gb.insertText("old text");
    try gb.setText("new text");

    var out: [32]u8 = undefined;
    const text = gb.getText(&out);
    try std.testing.expectEqualStrings("new text", text);
}
