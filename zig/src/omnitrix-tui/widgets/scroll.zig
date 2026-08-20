const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;

pub const ScrollState = struct {
    offset: i32 = 0,
    content_height: u16 = 0,
    viewport_height: u16 = 0,
    viewport_width: u16 = 0,
    auto_scroll: bool = true,
    show_scrollbar: bool = true,
    scrollbar_style: Style = .{ .fg = .{ .named = .bright_black } },
    track_style: Style = .{},
};

pub const ScrollContainer = struct {
    state: ScrollState,
    scroll_speed: u16,

    pub fn init(viewport_height: u16, viewport_width: u16) ScrollContainer {
        return .{
            .state = .{
                .viewport_height = viewport_height,
                .viewport_width = viewport_width,
                .auto_scroll = true,
            },
            .scroll_speed = 3,
        };
    }

    /// The view owns the current rectangle. Updating it never resets the user's
    /// position unless auto-scroll is active; otherwise the offset is only clamped.
    pub fn setViewport(self: *ScrollContainer, height: u16, width: u16) void {
        self.state.viewport_height = height;
        self.state.viewport_width = width;
        const max_offset = self.maxOffset();
        self.state.offset = if (self.state.auto_scroll) max_offset else @min(max_offset, @max(0, self.state.offset));
    }

    pub fn setContentHeight(self: *ScrollContainer, h: u16) void {
        self.state.content_height = h;
        if (self.state.auto_scroll) self.state.offset = self.maxOffset() else self.state.offset = @min(self.maxOffset(), @max(0, self.state.offset));
    }

    pub fn scrollUp(self: *ScrollContainer) void {
        self.state.offset = @max(0, self.state.offset - @as(i32, @intCast(self.scroll_speed)));
        self.state.auto_scroll = false;
    }

    pub fn scrollDown(self: *ScrollContainer) void {
        const max_offset = self.maxOffset();
        self.state.offset = @min(max_offset, self.state.offset + @as(i32, @intCast(self.scroll_speed)));
        if (self.state.offset >= max_offset) self.state.auto_scroll = true;
    }

    pub fn scrollToBottom(self: *ScrollContainer) void {
        self.state.offset = self.maxOffset();
        self.state.auto_scroll = true;
    }

    pub fn scrollToTop(self: *ScrollContainer) void {
        self.state.offset = 0;
        self.state.auto_scroll = false;
    }

    pub fn pageUp(self: *ScrollContainer) void {
        self.state.offset = @max(0, self.state.offset - @as(i32, @intCast(self.state.viewport_height)));
        self.state.auto_scroll = false;
    }

    pub fn pageDown(self: *ScrollContainer) void {
        const max_offset = self.maxOffset();
        self.state.offset = @min(max_offset, self.state.offset + @as(i32, @intCast(self.state.viewport_height)));
        if (self.state.offset >= max_offset) self.state.auto_scroll = true;
    }

    pub fn halfPageUp(self: *ScrollContainer) void {
        self.state.offset = @max(0, self.state.offset - @as(i32, @intCast(self.state.viewport_height / 2)));
        self.state.auto_scroll = false;
    }

    pub fn halfPageDown(self: *ScrollContainer) void {
        const max_offset = self.maxOffset();
        self.state.offset = @min(max_offset, self.state.offset + @as(i32, @intCast(self.state.viewport_height / 2)));
        if (self.state.offset >= max_offset) self.state.auto_scroll = true;
    }

    pub fn contentToScreen(self: ScrollContainer, content_y: u16) ?u16 {
        const sy = @as(i32, @intCast(content_y)) - self.state.offset;
        if (sy < 0 or sy >= @as(i32, @intCast(self.state.viewport_height))) return null;
        return @intCast(sy);
    }

    pub fn screenToContent(self: ScrollContainer, screen_y: u16) u16 {
        return @intCast(@max(0, @as(i32, @intCast(screen_y)) + self.state.offset));
    }

    pub fn renderScrollbar(self: ScrollContainer, buf: *Buffer, x: u16, y: u16, height: u16) void {
        if (!self.state.show_scrollbar or self.state.content_height <= self.state.viewport_height or height < 3) return;

        for (0..height) |i| {
            buf.setCell(x, y + @as(u16, @intCast(i)), .{
                .char = .{ .char = '│' },
                .style = self.state.track_style,
            });
        }

        const total = @as(usize, self.state.content_height);
        const view = @as(usize, self.state.viewport_height);
        const bar_height = @max(1, (height * view) / total);
        const scroll_ratio = if (total > view)
            @as(f32, @floatFromInt(@max(0, self.state.offset))) / @as(f32, @floatFromInt(total - view))
        else
            0;
        const thumb_pos = @as(usize, @intCast(@min(
            @as(i32, @intCast(height - bar_height)),
            @as(i32, @intFromFloat(scroll_ratio * @as(f32, @floatFromInt(height - bar_height)))),
        )));

        for (0..bar_height) |i| {
            if (thumb_pos + i < height) {
                buf.setCell(x, y + @as(u16, @intCast(thumb_pos + i)), .{
                    .char = .{ .char = '█' },
                    .style = self.state.scrollbar_style,
                });
            }
        }
    }

    pub fn clipRect(self: ScrollContainer, x: u16, y: u16, width: u16, height: u16) struct { x: u16, y: u16, width: u16, height: u16, content_offset: u16 } {
        _ = self;
        return .{ .x = x, .y = y, .width = width, .height = height, .content_offset = 0 };
    }

    fn maxOffset(self: ScrollContainer) i32 {
        return @max(0, @as(i32, @intCast(self.state.content_height)) - @as(i32, @intCast(self.state.viewport_height)));
    }
};
