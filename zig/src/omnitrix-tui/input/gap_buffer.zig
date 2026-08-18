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

    /// Delete byte before cursor (backspace)
    pub fn deleteBackward(self: *GapBuffer) ?u8 {
        if (self.gap_start == 0) return null;
        self.gap_start -= 1;
        self.len -= 1;
        return self.buf[self.gap_start];
    }

    /// Delete byte after cursor (delete key)
    pub fn deleteForward(self: *GapBuffer) ?u8 {
        if (self.gap_end >= self.buf.len) return null;
        const ch = self.buf[self.gap_end];
        self.gap_end += 1;
        self.len -= 1;
        return ch;
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

    /// Get all text as a contiguous slice (copies into provided buffer)
    pub fn getText(self: GapBuffer, out: []u8) []u8 {
        const before = self.buf[0..self.gap_start];
        const after_start = self.gap_end;
        const after_len = self.len - self.gap_start;
        const after = self.buf[after_start..][0..after_len];
        @memcpy(out[0..before.len], before);
        @memcpy(out[before.len..][0..after.len], after);
        return out[0..self.len];
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

    /// Move cursor left by one codepoint
    pub fn moveLeft(self: *GapBuffer) void {
        if (self.gap_start == 0) return;
        // Find the start of the previous UTF-8 char
        var pos = self.gap_start - 1;
        while (pos > 0 and (self.buf[pos] & 0xC0) == 0x80) {
            pos -= 1;
        }
        self.gap_start = pos;
        self.gap_end = self.buf.len - (self.len - self.gap_start);
        // Actually we need to use moveTo for proper gap movement
        self.moveTo(pos);
    }

    /// Move cursor right by one codepoint
    pub fn moveRight(self: *GapBuffer) void {
        if (self.gap_start >= self.len) return;
        // Find end of current UTF-8 char
        var end = self.gap_end;
        if (end < self.buf.len) {
            end += 1;
            while (end < self.buf.len and (self.buf[end] & 0xC0) == 0x80) {
                end += 1;
            }
        }
        const move_to = self.gap_start + 1;
        self.moveTo(move_to);
    }

    /// Move cursor to start of line
    pub fn moveToLineStart(self: *GapBuffer) void {
        var pos = self.gap_start;
        while (pos > 0) {
            pos -= 1;
            if (self.buf[pos] == '\n') {
                pos += 1;
                break;
            }
        }
        if (pos == 0 and self.buf[0] == '\n') {
            // already at start
        }
        self.moveTo(pos);
    }

    /// Move cursor to end of line
    pub fn moveToLineEnd(self: *GapBuffer) void {
        var pos = self.gap_start;
        while (pos < self.len) {
            const ch = self.charAt(pos) orelse break;
            if (ch == '\n') break;
            pos += 1;
        }
        self.moveTo(pos);
    }

    /// Move cursor to beginning
    pub fn moveToBeginning(self: *GapBuffer) void {
        self.moveTo(0);
    }

    /// Move cursor to end
    pub fn moveToEnd(self: *GapBuffer) void {
        self.moveTo(self.len);
    }

    /// Delete word backward (Ctrl+Backspace)
    pub fn deleteWordBackward(self: *GapBuffer) void {
        if (self.gap_start == 0) return;
        const old_pos = self.gap_start;
        var pos = old_pos;
        while (pos > 0 and self.buf[pos - 1] == ' ') pos -= 1;
        while (pos > 0 and self.buf[pos - 1] != ' ' and self.buf[pos - 1] != '\n') pos -= 1;
        self.deleteRange(pos, old_pos);
    }

    /// Delete word forward (Ctrl+Delete)
    pub fn deleteWordForward(self: *GapBuffer) void {
        if (self.gap_end >= self.buf.len) return;
        var end = self.gap_end;
        // Skip current word chars
        while (end < self.buf.len and self.buf[end] != ' ' and self.buf[end] != '\n') end += 1;
        // Skip whitespace
        while (end < self.buf.len and self.buf[end] == ' ') end += 1;
        self.gap_end = end;
        self.len = self.gap_start + (self.buf.len - self.gap_end);
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
