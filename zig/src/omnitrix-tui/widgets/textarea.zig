//! omnitrix-tui: Widget - TextArea (Çok Satırlı Metin Düzenleyici ve Kompozitör)
//!
//! Özellikler:
//! - Çok satırlı metin girişi ve düzenleme
//! - İmleç yönetimi (cursor_row, cursor_col)
//! - Karakter ekleme, silme (Backspace / Delete), satır bölme (Enter)
//! - Yön tuşları, Home, End ile imleç gezintisi
//! - Aktif imleç render'ı (▋ bloğu veya ters renkli hücre)

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const block_mod = @import("block.zig");

pub const Style = cell_mod.Style;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Block = block_mod.Block;

pub const TextArea = struct {
    allocator: std.mem.Allocator,
    lines: std.ArrayList(std.ArrayList(u8)),
    cursor_row: usize = 0,
    cursor_col: usize = 0,
    block: ?Block = null,
    style: Style = .default,
    cursor_style: Style = .{ .fg = .bright_cyan, .modifier = .{ .bold = true } },

    pub fn init(allocator: std.mem.Allocator) TextArea {
        var self = TextArea{
            .allocator = allocator,
            .lines = std.ArrayList(std.ArrayList(u8)).empty,
            .cursor_row = 0,
            .cursor_col = 0,
            .block = null,
            .style = .default,
            .cursor_style = .{ .fg = .bright_cyan, .modifier = .{ .bold = true } },
        };

        // İlk boş satır
        const first_line = std.ArrayList(u8).empty;
        self.lines.append(allocator, first_line) catch {};
        return self;
    }

    pub fn deinit(self: *TextArea) void {
        for (self.lines.items) |*l| {
            l.deinit(self.allocator);
        }
        self.lines.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn getText(self: *const TextArea, allocator: std.mem.Allocator) ![]u8 {
        var buf = std.ArrayList(u8).empty;
        errdefer buf.deinit(allocator);

        for (self.lines.items, 0..) |l, idx| {
            try buf.appendSlice(allocator, l.items);
            if (idx + 1 < self.lines.items.len) {
                try buf.append(allocator, '\n');
            }
        }
        return buf.toOwnedSlice(allocator);
    }

    pub fn setText(self: *TextArea, text: []const u8) !void {
        for (self.lines.items) |*l| l.deinit(self.allocator);
        self.lines.clearRetainingCapacity();

        var it = std.mem.splitScalar(u8, text, '\n');
        while (it.next()) |chunk| {
            var l = std.ArrayList(u8).empty;
            try l.appendSlice(self.allocator, chunk);
            try self.lines.append(self.allocator, l);
        }

        if (self.lines.items.len == 0) {
            try self.lines.append(self.allocator, std.ArrayList(u8).empty);
        }

        self.cursor_row = self.lines.items.len - 1;
        self.cursor_col = self.lines.items[self.cursor_row].items.len;
    }

    pub fn clear(self: *TextArea) void {
        for (self.lines.items) |*l| l.deinit(self.allocator);
        self.lines.clearRetainingCapacity();
        const l = std.ArrayList(u8).empty;
        self.lines.append(self.allocator, l) catch {};
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    pub fn handleKey(self: *TextArea, key: []const u8) !bool {
        if (key.len == 0) return false;

        // Enter
        if (key.len == 1 and (key[0] == '\r' or key[0] == '\n')) {
            var cur_line = &self.lines.items[self.cursor_row];
            var new_line = std.ArrayList(u8).empty;

            if (self.cursor_col < cur_line.items.len) {
                try new_line.appendSlice(self.allocator, cur_line.items[self.cursor_col..]);
                cur_line.shrinkRetainingCapacity(self.cursor_col);
            }

            try self.lines.insert(self.allocator, self.cursor_row + 1, new_line);
            self.cursor_row += 1;
            self.cursor_col = 0;
            return true;
        }

        // Backspace
        if (key.len == 1 and (key[0] == 127 or key[0] == 8)) {
            var cur_line = &self.lines.items[self.cursor_row];
            if (self.cursor_col > 0) {
                _ = cur_line.orderedRemove(self.cursor_col - 1);
                self.cursor_col -= 1;
                return true;
            } else if (self.cursor_row > 0) {
                // Önceki satırla birleştir
                const prev_len = self.lines.items[self.cursor_row - 1].items.len;
                try self.lines.items[self.cursor_row - 1].appendSlice(self.allocator, cur_line.items);
                cur_line.deinit(self.allocator);
                _ = self.lines.orderedRemove(self.cursor_row);
                self.cursor_row -= 1;
                self.cursor_col = prev_len;
                return true;
            }
            return false;
        }

        // Ok Tuşları
        if (key.len == 3 and key[0] == '\x1b' and key[1] == '[') {
            switch (key[2]) {
                'A' => { // Up
                    if (self.cursor_row > 0) {
                        self.cursor_row -= 1;
                        self.cursor_col = @min(self.cursor_col, self.lines.items[self.cursor_row].items.len);
                        return true;
                    }
                },
                'B' => { // Down
                    if (self.cursor_row + 1 < self.lines.items.len) {
                        self.cursor_row += 1;
                        self.cursor_col = @min(self.cursor_col, self.lines.items[self.cursor_row].items.len);
                        return true;
                    }
                },
                'C' => { // Right
                    if (self.cursor_col < self.lines.items[self.cursor_row].items.len) {
                        self.cursor_col += 1;
                        return true;
                    } else if (self.cursor_row + 1 < self.lines.items.len) {
                        self.cursor_row += 1;
                        self.cursor_col = 0;
                        return true;
                    }
                },
                'D' => { // Left
                    if (self.cursor_col > 0) {
                        self.cursor_col -= 1;
                        return true;
                    } else if (self.cursor_row > 0) {
                        self.cursor_row -= 1;
                        self.cursor_col = self.lines.items[self.cursor_row].items.len;
                        return true;
                    }
                },
                else => {},
            }
        }

        // Normal karakter yazma
        if (key.len == 1 and key[0] >= 32 and key[0] <= 126) {
            var cur_line = &self.lines.items[self.cursor_row];
            try cur_line.insert(self.allocator, self.cursor_col, key[0]);
            self.cursor_col += 1;
            return true;
        }

        return false;
    }

    pub fn render(self: *const TextArea, area: Rect, buf: *Buffer, is_focused: bool) void {
        if (area.isEmpty()) return;

        var render_area = area;
        if (self.block) |b| {
            b.render(area, buf);
            render_area = b.inner(area);
        }

        if (render_area.isEmpty()) return;

        for (self.lines.items, 0..) |line, row_idx| {
            if (row_idx >= render_area.height) break;
            const y = render_area.top() + @as(u16, @intCast(row_idx));

            _ = buf.setString(render_area.left(), y, line.items, self.style, render_area.width);

            // İmleci çiz
            if (is_focused and row_idx == self.cursor_row) {
                const cur_x = render_area.left() + @as(u16, @intCast(self.cursor_col));
                if (cur_x < render_area.right()) {
                    if (buf.getMut(cur_x, y)) |c| {
                        c.setSymbol("▋", 1);
                        c.setStyle(self.cursor_style);
                    }
                }
            }
        }
    }
};

test "textarea metin girisi, backspace ve render" {
    const area = Rect.init(0, 0, 40, 5);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var ta = TextArea.init(std.testing.allocator);
    defer ta.deinit();

    _ = try ta.handleKey("A");
    _ = try ta.handleKey("B");
    _ = try ta.handleKey("C");

    const txt = try ta.getText(std.testing.allocator);
    defer std.testing.allocator.free(txt);
    try std.testing.expectEqualStrings("ABC", txt);

    _ = try ta.handleKey("\x7f"); // Backspace
    const txt2 = try ta.getText(std.testing.allocator);
    defer std.testing.allocator.free(txt2);
    try std.testing.expectEqualStrings("AB", txt2);

    ta.render(area, &buf, true);
    const c0 = buf.get(0, 0).?;
    try std.testing.expectEqualStrings("A", c0.getSymbol());
}
