const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const layout_mod = @import("../core/layout.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;
const Rect = layout_mod.Rect;
const stringWidth = buffer_mod.stringWidth;

pub const ListItem = struct {
    label: []const u8,
    value: ?[]const u8 = null,
    style: Style = .{},
    disabled: bool = false,
};

pub const ListWidget = struct {
    items: []const ListItem,
    selected: i32,
    scroll_offset: u16,
    item_style: Style,
    selected_style: Style,
    disabled_style: Style,
    highlight_style: Style,

    pub fn init(items: []const ListItem, item_style: Style, selected_style: Style) ListWidget {
        return .{
            .items = items,
            .selected = 0,
            .scroll_offset = 0,
            .item_style = item_style,
            .selected_style = selected_style,
            .disabled_style = .{ .fg = .{ .named = .bright_black } },
            .highlight_style = .{ .fg = .{ .named = .bright_blue } },
        };
    }

    pub fn moveUp(self: *ListWidget) void {
        if (self.items.len == 0) return;
        if (self.selected > 0) {
            self.selected -= 1;
        } else {
            self.selected = @intCast(self.items.len - 1);
        }
    }

    pub fn moveDown(self: *ListWidget) void {
        if (self.items.len == 0) return;
        if (self.selected < @as(i32, @intCast(self.items.len)) - 1) {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
    }

    pub fn pageUp(self: *ListWidget, page_size: u16) void {
        self.selected = @max(0, self.selected - @as(i32, @intCast(page_size)));
    }

    pub fn pageDown(self: *ListWidget, page_size: u16) void {
        self.selected = @min(@as(i32, @intCast(self.items.len)) - 1, self.selected + @as(i32, @intCast(page_size)));
    }

    pub fn moveHome(self: *ListWidget) void {
        self.selected = 0;
    }

    pub fn moveEnd(self: *ListWidget) void {
        if (self.items.len > 0) {
            self.selected = @intCast(self.items.len - 1);
        }
    }

    pub fn selectedValue(self: ListWidget) ?[]const u8 {
        if (self.items.len == 0) return null;
        const idx: usize = @intCast(@max(0, @min(self.selected, @as(i32, @intCast(self.items.len)) - 1)));
        return self.items[idx].value orelse self.items[idx].label;
    }

    /// Calculate visible items and render
    pub fn render(self: *ListWidget, buf: *Buffer, rect: Rect, _: ?[]const u8) void {
        const visible_height = rect.height;
        if (visible_height == 0) return;

        // Ensure selected is in view
        const sel: usize = @intCast(@max(0, self.selected));
        if (sel < self.scroll_offset) {
            self.scroll_offset = @intCast(sel);
        } else if (sel >= self.scroll_offset + visible_height) {
            self.scroll_offset = @intCast(sel - visible_height + 1);
        }

        var visible_idx: u16 = 0;
        var item_idx: usize = 0;
        for (self.items) |item| {
            if (visible_idx >= visible_height) break;
            if (item_idx < self.scroll_offset) {
                item_idx += 1;
                continue;
            }

            const row = rect.y + visible_idx;
            const is_selected = @as(i32, @intCast(item_idx)) == self.selected;

            const style = if (item.disabled)
                self.disabled_style
            else if (is_selected)
                self.selected_style
            else
                item.style;

            // Fill entire row with style
            for (0..rect.width) |col| {
                buf.setCell(rect.x + @as(u16, @intCast(col)), row, .{ .style = if (is_selected) self.selected_style else style });
            }

            // Selection indicator
            if (is_selected) {
                buf.setCell(rect.x, row, .{ .char = .{ .char = '›' }, .style = self.highlight_style });
            }

            // Item label
            const label_style = if (is_selected) self.selected_style else style;
            const max_label_w = rect.width - 2; // leave space for indicator
            _ = buf.writeStringBounded(rect.x + 1, row, item.label, label_style, max_label_w);

            item_idx += 1;
            visible_idx += 1;
        }

        // Fill remaining rows
        while (visible_idx < visible_height) {
            const row = rect.y + visible_idx;
            for (0..rect.width) |col| {
                buf.setCell(rect.x + @as(u16, @intCast(col)), row, .{ .style = self.item_style });
            }
            visible_idx += 1;
        }
    }
};
