const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const gap_mod = @import("../input/gap_buffer.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const GapBuffer = gap_mod.GapBuffer;
const stringWidth = buffer_mod.stringWidth;
const unicodeWidth = buffer_mod.charWidth;
const unicode_helper = @import("../core/unicode.zig");
const charWidth = buffer_mod.charWidth;
const Rect = @import("../core/layout.zig").Rect;

pub const TextareaWidget = struct {
    gap: GapBuffer,
    cursor_visible: bool,
    scroll_x: u16,
    scroll_y: u16,
    style: Style,
    cursor_style: Style,
    placeholder: ?[]const u8,
    placeholder_style: Style,
    max_lines: u16,
    line_widths: std.ArrayList(u16),
    multiline: bool,
    allocator: std.mem.Allocator,
    max_bytes: usize,

    pub fn init(allocator: std.mem.Allocator, style: Style, cursor_style: Style) !TextareaWidget {
        return .{
            .gap = try GapBuffer.init(allocator, 256),
            .cursor_visible = true,
            .scroll_x = 0,
            .scroll_y = 0,
            .style = style,
            .cursor_style = cursor_style,
            .placeholder = null,
            .placeholder_style = .{ .fg = .{ .named = .bright_black } },
            .max_lines = 1,
            .line_widths = .empty,
            .multiline = false,
            .allocator = allocator,
            .max_bytes = 512,
        };
    }

    pub fn deinit(self: *TextareaWidget) void {
        self.gap.deinit();
        self.line_widths.deinit(self.allocator);
    }

    pub fn setText(self: *TextareaWidget, text: []const u8) !void {
        try self.gap.setText(text);
        self.recalcLines();
    }

    pub fn getText(self: *TextareaWidget) ![]u8 {
        return try self.gap.getTextOwned();

    }

    pub fn snapshot(self: TextareaWidget, out: []u8) []u8 {
        return self.gap.getTextBounded(out);
    }

    pub fn isEmpty(self: TextareaWidget) bool {
        return self.gap.isEmpty();
    }

    pub fn handleKey(self: *TextareaWidget, key: struct {
        char: ?u21 = null,
        key: enum {
            none,
            enter,
            backspace,
            delete,
            left,
            right,
            up,
            down,
            home,
            end,
            ctrl_backspace,
            ctrl_delete,
            ctrl_home,
            ctrl_end,
        } = .none,
        ctrl: bool = false,
        alt: bool = false,
    }) !void {
        if (key.char) |ch| {
            if (key.ctrl or key.alt) return;
            var encoded: [4]u8 = undefined;
            const encoded_len = std.unicode.wtf8Encode(ch, &encoded) catch 1;
            if (self.gap.length() + encoded_len > self.max_bytes) return;
            if (key.ctrl or key.alt) return;
            if (ch == '\n' or ch == '\r') {
                if (self.multiline) {
                    try self.gap.insertCodepoint('\n');
                }
            } else {
                try self.gap.insertCodepoint(ch);
            }
            self.recalcLines();
            return;
        }

        switch (key.key) {
            .enter => {
                if (self.multiline and self.gap.length() < self.max_bytes) {
                    try self.gap.insertCodepoint('\n');
                }
            },
            .backspace => {
                if (key.ctrl) {
                    self.deleteWordBackward();
                } else {
                    _ = self.gap.deleteBackward();
                }
            },
            .delete => {
                if (key.ctrl) {
                    self.gap.deleteWordForward();
                } else {
                    _ = self.gap.deleteForward();
                }
            },
            .left => {
                if (key.ctrl) {
                    self.moveWordLeft();
                } else {
                    self.gap.moveLeft();
                }
            },
            .right => {
                if (key.ctrl) {
                    self.moveWordRight();
                } else {
                    self.gap.moveRight();
                }
            },
            .up => {
                if (self.multiline) self.moveLineUp();
            },
            .down => {
                if (self.multiline) self.moveLineDown();
            },
            .home => {
                if (key.ctrl) {
                    self.gap.moveToBeginning();
                } else {
                    self.gap.moveToLineStart();
                }
            },
            .end => {
                if (key.ctrl) {
                    self.gap.moveToEnd();
                } else {
                    self.gap.moveToLineEnd();
                }
            },
            .ctrl_backspace => self.deleteWordBackward(),
            .ctrl_delete => self.gap.deleteWordForward(),
            .ctrl_home => self.gap.moveToBeginning(),
            .ctrl_end => self.gap.moveToEnd(),
            .none => {},
        }
        self.recalcLines();
    }

    fn deleteWordBackward(self: *TextareaWidget) void {
        const cursor = self.gap.normalizeBoundary(self.gap.cursor());
        if (cursor == 0) return;
        var pos = cursor;
        while (pos > 0) {
            const previous = self.gap.previousBoundary(pos);
            if (!self.gap.isSeparator(previous)) break;
            pos = previous;
        }
        while (pos > 0) {
            const previous = self.gap.previousBoundary(pos);
            if (self.gap.isSeparator(previous)) break;
            pos = previous;
        }
        self.gap.deleteRange(pos, cursor);
    }

    fn moveWordLeft(self: *TextareaWidget) void {
        var pos = self.gap.normalizeBoundary(self.gap.cursor());
        if (pos == 0) return;
        while (pos > 0) {
            const previous = self.gap.previousBoundary(pos);
            if (!self.gap.isSeparator(previous)) break;
            pos = previous;
        }
        while (pos > 0) {
            const previous = self.gap.previousBoundary(pos);
            if (self.gap.isSeparator(previous)) break;
            pos = previous;
        }
        self.gap.moveTo(pos);
    }

    fn moveWordRight(self: *TextareaWidget) void {
        const len = self.gap.length();
        var pos = self.gap.normalizeBoundary(self.gap.cursor());
        while (pos < len and !self.gap.isSeparator(pos)) pos = self.gap.nextBoundary(pos);
        while (pos < len and self.gap.isSeparator(pos)) pos = self.gap.nextBoundary(pos);
        self.gap.moveTo(pos);
    }

    fn lineStart(self: TextareaWidget, pos: usize) usize {
        var current = self.gap.normalizeBoundary(pos);
        while (current > 0) {
            const previous = self.gap.previousBoundary(current);
            const item = self.gap.codepointAt(previous) orelse break;
            if (item.cp == '\n') break;
            current = previous;
        }
        return current;
    }

    fn lineEnd(self: TextareaWidget, pos: usize) usize {
        var current = self.gap.normalizeBoundary(pos);
        while (current < self.gap.length()) {
            const item = self.gap.codepointAt(current) orelse break;
            if (item.cp == '\n') break;
            current += item.len;
        }
        return current;
    }

    fn displayColumn(self: TextareaWidget, start: usize, end: usize) u16 {
        var current = start;
        var column: u16 = 0;
        while (current < end) {
            const item = self.gap.codepointAt(current) orelse break;
            if (item.cp == '\n') break;
            column +|= charWidth(item.cp);
            current += item.len;
        }
        return column;
    }

    fn boundaryAtColumn(self: TextareaWidget, start: usize, end: usize, target: u16) usize {
        var current = start;
        var column: u16 = 0;
        while (current < end) {
            const item = self.gap.codepointAt(current) orelse break;
            if (item.cp == '\n') break;
            const width = charWidth(item.cp);
            const next_column = column +| width;
            if (next_column > target) break;
            column = next_column;
            current += item.len;
        }
        return current;
    }

    fn moveLineUp(self: *TextareaWidget) void {
        const cursor = self.gap.normalizeBoundary(self.gap.cursor());
        const current_start = self.lineStart(cursor);
        if (current_start == 0) return;

        const previous_line_end = self.gap.previousBoundary(current_start);
        const previous_line_start = self.lineStart(previous_line_end);
        const column = self.displayColumn(current_start, cursor);
        self.gap.moveTo(self.boundaryAtColumn(previous_line_start, previous_line_end, column));
    }

    fn moveLineDown(self: *TextareaWidget) void {
        const cursor = self.gap.normalizeBoundary(self.gap.cursor());
        const current_start = self.lineStart(cursor);
        const current_end = self.lineEnd(cursor);
        if (current_end >= self.gap.length()) return;

        const next_line_start = self.gap.nextBoundary(current_end);
        const next_line_end = self.lineEnd(next_line_start);
        const column = self.displayColumn(current_start, cursor);
        self.gap.moveTo(self.boundaryAtColumn(next_line_start, next_line_end, column));
    }

    fn recalcLines(self: *TextareaWidget) void {
        self.line_widths.clearRetainingCapacity();
        var current_width: u16 = 0;
        var buf: [512]u8 = undefined;
        const text = self.gap.getTextBounded(&buf);
        var i: usize = 0;
        while (i < text.len) {
            if (text[i] == '\n') {
                self.line_widths.append(self.allocator, current_width) catch {};
                current_width = 0;
                i += 1;
                continue;
            }
            const result = unicode_helper.decodeCodepoint(text, i);
            current_width += unicodeWidth(result.cp);
            i += result.len;
        }
        self.line_widths.append(self.allocator, current_width) catch {};
    }

    pub fn lineCount(self: TextareaWidget) u16 {
        return @max(1, @as(u16, @intCast(self.line_widths.items.len)));
    }

    pub fn cursorPosition(self: TextareaWidget, rect_width: u16) struct { col: u16, row: u16 } {
        const cursor = self.gap.cursor();
        var current_row: u16 = 0;
        var current_col: u16 = 0;
        var i: usize = 0;
        var buf: [512]u8 = undefined;
        const text = self.gap.getTextBounded(&buf);

        while (i < text.len and i < cursor) {
            if (text[i] == '\n') {
                current_row += 1;
                current_col = 0;
                i += 1;
                continue;
            }
            const result = unicode_helper.decodeCodepoint(text, i);
            current_col += charWidth(result.cp);
            i += result.len;
        }

        if (rect_width > 0) {
            const visual_row = current_col / rect_width;
            current_row += @intCast(visual_row);
            current_col = current_col % rect_width;
        }

        return .{ .col = current_col, .row = current_row };
    }

    pub fn render(self: *TextareaWidget, buf: *Buffer, rect: Rect) void {
        buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = self.style });

        const text_len = self.gap.length();
        if (text_len == 0) {
            if (self.placeholder) |ph| {
                _ = buf.writeStringBounded(rect.x, rect.y, ph, self.placeholder_style, rect.width);
            }
            return;
        }

        var text_buf: [512]u8 = undefined;
        const text = self.gap.getTextBounded(&text_buf);

        var x: u16 = 0;
        var y: u16 = 0;
        var i: usize = 0;

        while (i < text.len and y < rect.height) {
            if (text[i] == '\n') {
                while (x < rect.width) {
                    buf.setCell(rect.x + x, rect.y + y, .{ .style = self.style });
                    x += 1;
                }
                x = 0;
                y += 1;
                i += 1;
                continue;
            }

            const result = unicode_helper.decodeCodepoint(text, i);
            const cw = charWidth(result.cp);
            if (x + @as(u16, @intCast(cw)) > rect.width) {
                while (x < rect.width) {
                    buf.setCell(rect.x + x, rect.y + y, .{ .style = self.style });
                    x += 1;
                }
                x = 0;
                y += 1;
                if (y >= rect.height) break;
                continue;
            }
            buf.setCell(rect.x + x, rect.y + y, .{ .char = .{ .char = result.cp }, .style = self.style, .width = @intCast(cw) });
            if (cw == 2) buf.setCell(rect.x + x + 1, rect.y + y, .{ .char = .wide_right, .style = self.style, .width = 0 });
            x += @intCast(cw);
            i += result.len;
        }

        while (y < rect.height) {
            while (x < rect.width) {
                buf.setCell(rect.x + x, rect.y + y, .{ .style = self.style });
                x += 1;
            }
            x = 0;
            y += 1;
        }

        if (self.cursor_visible) {
            const cpos = self.cursorPosition(rect.width);
            if (cpos.row < rect.height and cpos.col < rect.width) {
                const cx = rect.x + cpos.col;
                const cy = rect.y + cpos.row;
                const existing = buf.getCell(cx, cy);
                buf.setCell(cx, cy, .{
                    .char = existing.char,
                    .style = self.cursor_style,
                    .width = existing.width,
                });
            }
        }
    }
};
