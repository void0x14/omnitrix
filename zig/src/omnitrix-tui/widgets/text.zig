const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const unicode_helper = @import("../core/unicode.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;
const stringWidth = buffer_mod.stringWidth;
const charWidth = buffer_mod.charWidth;

pub const TextAlign = enum { left, center, right };

pub const TextWidget = struct {
    text: []const u8,
    style: Style,
    align_: TextAlign,
    max_width: u16,

    pub fn init(text: []const u8, style: Style) TextWidget {
        return .{ .text = text, .style = style, .align_ = .left, .max_width = 65535 };
    }

    pub fn lineCount(self: TextWidget, available_width: u16) u16 {
        const w = @min(self.max_width, available_width);
        if (w == 0) return 0;
        var lines: u16 = 0;
        var current_width: u16 = 0;
        var i: usize = 0;
        while (i < self.text.len) {
            if (self.text[i] == '\n') { lines += 1; current_width = 0; i += 1; continue; }
            const result = unicode_helper.decodeCodepoint(self.text, i);
            const cw = charWidth(result.cp);
            if (current_width + @as(u16, @intCast(cw)) > w) { lines += 1; current_width = 0; }
            current_width += @intCast(cw);
            i += result.len;
        }
        if (current_width > 0 or self.text.len == 0) lines += 1;
        return lines;
    }

    pub fn render(self: TextWidget, buf: *Buffer, x: u16, y: u16, available_width: u16) void {
        const max_w = @min(self.max_width, available_width);
        var line_x: u16 = 0;
        var line_y: u16 = 0;
        var i: usize = 0;
        while (i < self.text.len) {
            if (self.text[i] == '\n') {
                const row = y + line_y;
                while (line_x < max_w) { buf.setCell(x + line_x, row, .{ .style = self.style }); line_x += 1; }
                line_x = 0; line_y += 1; i += 1; continue;
            }
            const result = unicode_helper.decodeCodepoint(self.text, i);
            const cw = charWidth(result.cp);
            if (line_x + @as(u16, @intCast(cw)) > max_w) {
                const row = y + line_y;
                while (line_x < max_w) { buf.setCell(x + line_x, row, .{ .style = self.style }); line_x += 1; }
                line_x = 0; line_y += 1; continue;
            }
            const row = y + line_y;
            buf.setCell(x + line_x, row, .{ .char = .{ .char = result.cp }, .style = self.style, .width = @intCast(cw) });
            if (cw == 2) buf.setCell(x + line_x + 1, row, .{ .char = .wide_right, .style = self.style, .width = 0 });
            line_x += @intCast(cw);
            i += result.len;
        }
        const row = y + line_y;
        while (line_x < max_w) { buf.setCell(x + line_x, row, .{ .style = self.style }); line_x += 1; }
    }
};
