const std = @import("std");
const cell_mod = @import("cell.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const unicode_helper = @import("unicode.zig");

pub const Buffer = struct {
    allocator: std.mem.Allocator,
    cols: u16,
    rows: u16,
    front: []Cell,
    back: []Cell,

    pub fn init(allocator: std.mem.Allocator, cols: u16, rows: u16) !Buffer {
        const total = @as(usize, cols) * @as(usize, rows);
        const front = try allocator.alloc(Cell, total);
        const back = try allocator.alloc(Cell, total);
        @memset(front, Cell{});
        @memset(back, Cell{});
        return .{ .allocator = allocator, .cols = cols, .rows = rows, .front = front, .back = back };
    }

    pub fn deinit(self: *Buffer) void {
        self.allocator.free(self.front);
        self.allocator.free(self.back);
    }

    pub fn resize(self: *Buffer, new_cols: u16, new_rows: u16) !void {
        const new_total = @as(usize, new_cols) * @as(usize, new_rows);
        const old_total = @as(usize, self.cols) * @as(usize, self.rows);
        if (new_total == old_total) {
            self.cols = new_cols;
            self.rows = new_rows;
            return;
        }
        const new_front = try self.allocator.alloc(Cell, new_total);
        const new_back = try self.allocator.alloc(Cell, new_total);
        @memset(new_front, Cell{});
        @memset(new_back, Cell{});
        const min_rows = @min(self.rows, new_rows);
        const min_cols = @min(self.cols, new_cols);
        for (0..min_rows) |row| {
            const old_start = @as(usize, row) * self.cols;
            const new_start = @as(usize, row) * new_cols;
            for (0..min_cols) |col| {
                new_back[new_start + col] = self.back[old_start + col];
                new_front[new_start + col] = self.front[old_start + col];
            }
        }
        self.allocator.free(self.front);
        self.allocator.free(self.back);
        self.front = new_front;
        self.back = new_back;
        self.cols = new_cols;
        self.rows = new_rows;
    }

    pub fn totalCells(self: Buffer) usize {
        return @as(usize, self.cols) * @as(usize, self.rows);
    }

    pub fn setCell(self: *Buffer, col: u16, row: u16, c: Cell) void {
        if (col >= self.cols or row >= self.rows) return;
        const idx = @as(usize, row) * self.cols + col;
        self.back[idx] = c;
    }

    pub fn getCell(self: Buffer, col: u16, row: u16) Cell {
        if (col >= self.cols or row >= self.rows) return Cell{};
        const idx = @as(usize, row) * self.cols + col;
        return self.back[idx];
    }

    pub fn clear(self: *Buffer) void {
        @memset(self.back, Cell{});
    }

    pub fn fillRegion(self: *Buffer, x: u16, y: u16, w: u16, h: u16, c: Cell) void {
        for (0..h) |dy| {
            for (0..w) |dx| {
                const col = x + @as(u16, @intCast(dx));
                const row = y + @as(u16, @intCast(dy));
                if (col < self.cols and row < self.rows) {
                    self.setCell(col, row, c);
                }
            }
        }
    }

    pub fn writeString(self: *Buffer, start_x: u16, y: u16, text: []const u8, style: Style) u16 {
        var x = start_x;
        var i: usize = 0;
        while (i < text.len) {
            if (text[i] == '\n') {
                while (x < self.cols) { self.setCell(x, y, .{ .style = style }); x += 1; }
                return x;
            }
            const result = unicode_helper.decodeCodepoint(text, i);
            const char_width = charWidth(result.cp);
            if (x + @as(u16, @intCast(char_width)) > self.cols) break;
            self.setCell(x, y, .{ .char = .{ .char = result.cp }, .style = style, .width = @intCast(char_width) });
            if (char_width == 2) {
                self.setCell(x + 1, y, .{ .char = .wide_right, .style = style, .width = 0 });
            }
            x += @intCast(char_width);
            i += result.len;
        }
        return x;
    }

    pub fn writeStringBounded(self: *Buffer, start_x: u16, y: u16, text: []const u8, style: Style, max_width: u16) u16 {
        var x = start_x;
        var remaining = max_width;
        var i: usize = 0;
        while (i < text.len and remaining > 0) {
            if (text[i] == '\n') break;
            const result = unicode_helper.decodeCodepoint(text, i);
            const char_width = charWidth(result.cp);
            if (@as(u16, @intCast(char_width)) > remaining) break;
            self.setCell(x, y, .{ .char = .{ .char = result.cp }, .style = style, .width = @intCast(char_width) });
            if (char_width == 2) {
                self.setCell(x + 1, y, .{ .char = .wide_right, .style = style, .width = 0 });
            }
            x += @intCast(char_width);
            remaining -= @intCast(char_width);
            i += result.len;
        }
        while (remaining > 0) { self.setCell(x, y, .{ .style = style }); x += 1; remaining -= 1; }
        return x;
    }

    pub fn flush(self: *Buffer, writer: anytype) !void {
        var buf: [128]u8 = undefined;
        var last_style: ?Style = null;
        var i: usize = 0;
        const total = self.totalCells();
        while (i < total) : (i += 1) {
            if (self.front[i].eq(self.back[i])) continue;
            const row: u16 = @intCast(i / self.cols);
            const col: u16 = @intCast(i % self.cols);
            const pos_str = std.fmt.bufPrint(&buf, "\x1b[{d};{d}H", .{ row + 1, col + 1 }) catch continue;
            try writer.writeAll(pos_str);
            if (last_style == null or !last_style.?.eq(self.back[i].style)) {
                try writer.writeAll("\x1b[0m");
                var fg_buf: [32]u8 = undefined;
                try writer.writeAll(self.back[i].style.fg.toAnsiFg(&fg_buf));
                try writer.writeAll(self.back[i].style.bg.toAnsiBg(&fg_buf));
                var attr_buf: [32]u8 = undefined;
                try writer.writeAll(self.back[i].style.attr.toAnsi(&attr_buf));
                last_style = self.back[i].style;
            }
            var char_buf: [4]u8 = undefined;
            try writer.writeAll(self.back[i].writeUtf8(&char_buf));
            self.front[i] = self.back[i];
        }
        try writer.writeAll("\x1b[0m");
    }

    pub fn invalidate(self: *Buffer) void {
        @memset(self.front, Cell{});
    }
};

pub fn charWidth(cp: u21) u8 {
    if (cp < 0x20 or (cp >= 0x7f and cp < 0xa0)) return 0;
    if (cp == 0x20 or cp == 0xa0 or cp == 0x200b) return 0;
    if (cp == 0x08 or cp == 0x0a or cp == 0x0d) return 0;
    if ((cp >= 0x1100 and cp <= 0x115f) or
        (cp >= 0x2e80 and cp <= 0xa4cf and cp != 0x303f) or
        (cp >= 0xac00 and cp <= 0xd7a3) or
        (cp >= 0xf900 and cp <= 0xfaff) or
        (cp >= 0xfe10 and cp <= 0xfe19) or
        (cp >= 0xfe30 and cp <= 0xfe6f) or
        (cp >= 0xff00 and cp <= 0xff60) or
        (cp >= 0xffe0 and cp <= 0xffe6) or
        (cp >= 0x20000 and cp <= 0x2fffd))
        return 2;
    return 1;
}

pub fn stringWidth(text: []const u8) u16 {
    var width: u16 = 0;
    var i: usize = 0;
    while (i < text.len) {
        const result = unicode_helper.decodeCodepoint(text, i);
        width += charWidth(result.cp);
        i += result.len;
    }
    return width;
}

test "unicodeWidth" {
    try std.testing.expectEqual(@as(u8, 1), charWidth('A'));
    try std.testing.expectEqual(@as(u8, 2), charWidth(0x4e16));
}

test "stringWidth" {
    try std.testing.expectEqual(@as(u16, 5), stringWidth("Hello"));
}
