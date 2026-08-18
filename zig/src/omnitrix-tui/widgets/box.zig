const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const layout_mod = @import("../core/layout.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const Rect = layout_mod.Rect;

pub const BorderStyle = enum { none, single, double, rounded, thick, full_block };

pub const BorderChars = struct {
    top_left: u21,
    top_right: u21,
    bottom_left: u21,
    bottom_right: u21,
    horizontal: u21,
    vertical: u21,
};

pub fn getBorderChars(bs: BorderStyle) ?BorderChars {
    return switch (bs) {
        .none => null,
        .single => .{
            .top_left = '┌',
            .top_right = '┐',
            .bottom_left = '└',
            .bottom_right = '┘',
            .horizontal = '─',
            .vertical = '│',
        },
        .double => .{
            .top_left = '╔',
            .top_right = '╗',
            .bottom_left = '╚',
            .bottom_right = '╝',
            .horizontal = '═',
            .vertical = '║',
        },
        .rounded => .{
            .top_left = '╭',
            .top_right = '╮',
            .bottom_left = '╰',
            .bottom_right = '╯',
            .horizontal = '─',
            .vertical = '│',
        },
        .thick => .{
            .top_left = '┏',
            .top_right = '┓',
            .bottom_left = '┗',
            .bottom_right = '┛',
            .horizontal = '━',
            .vertical = '┃',
        },
        .full_block => .{
            .top_left = '█',
            .top_right = '█',
            .bottom_left = '█',
            .bottom_right = '█',
            .horizontal = '█',
            .vertical = '█',
        },
    };
}

pub const BoxWidget = struct {
    border: BorderStyle,
    border_style: Style,
    title: ?[]const u8,
    title_style: Style,
    bg_style: Style,

    pub fn init(border: BorderStyle, border_style: Style, bg_style: Style) BoxWidget {
        return .{
            .border = border,
            .border_style = border_style,
            .title = null,
            .title_style = border_style,
            .bg_style = bg_style,
        };
    }

    pub fn withTitle(self: BoxWidget, title: []const u8, title_style: Style) BoxWidget {
        var b = self;
        b.title = title;
        b.title_style = title_style;
        return b;
    }

    /// Get the inner content area (inside borders and padding)
    pub fn innerRect(self: BoxWidget, outer: Rect) Rect {
        if (self.border == .none) return outer;
        return outer.shrink(1, 1, 1, 1);
    }

    /// Render border and background
    pub fn render(self: BoxWidget, buf: *Buffer, rect: Rect) void {
        // Fill background
        buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = self.bg_style });

        if (self.border == .none) return;
        const chars = getBorderChars(self.border) orelse return;

        if (rect.width < 2 or rect.height < 2) return;

        // Corners
        buf.setCell(rect.x, rect.y, .{ .char = .{ .char = chars.top_left }, .style = self.border_style });
        buf.setCell(rect.x + rect.width - 1, rect.y, .{ .char = .{ .char = chars.top_right }, .style = self.border_style });
        buf.setCell(rect.x, rect.y + rect.height - 1, .{ .char = .{ .char = chars.bottom_left }, .style = self.border_style });
        buf.setCell(rect.x + rect.width - 1, rect.y + rect.height - 1, .{ .char = .{ .char = chars.bottom_right }, .style = self.border_style });

        // Horizontal edges
        for (1..rect.width - 1) |i| {
            buf.setCell(rect.x + @as(u16, @intCast(i)), rect.y, .{ .char = .{ .char = chars.horizontal }, .style = self.border_style });
            buf.setCell(rect.x + @as(u16, @intCast(i)), rect.y + rect.height - 1, .{ .char = .{ .char = chars.horizontal }, .style = self.border_style });
        }

        // Vertical edges
        for (1..rect.height - 1) |i| {
            buf.setCell(rect.x, rect.y + @as(u16, @intCast(i)), .{ .char = .{ .char = chars.vertical }, .style = self.border_style });
            buf.setCell(rect.x + rect.width - 1, rect.y + @as(u16, @intCast(i)), .{ .char = .{ .char = chars.vertical }, .style = self.border_style });
        }

        // Title
        if (self.title) |title| {
            if (rect.width > 4) {
                const title_w = buffer_mod.stringWidth(title);
                const max_title_w = rect.width - 4;
                const tw = @min(title_w, max_title_w);
                const tx = rect.x + 2;
                // Clear border under title
                for (0..tw + 2) |i| {
                    buf.setCell(tx + @as(u16, @intCast(i)), rect.y, .{ .style = self.bg_style });
                }
                _ = buf.writeString(tx, rect.y, title[0..@min(title.len, max_title_w)], self.title_style);
            }
        }
    }
};
