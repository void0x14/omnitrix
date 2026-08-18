const std = @import("std");

pub const Rect = struct {
    x: u16,
    y: u16,
    width: u16,
    height: u16,

    pub const empty: Rect = .{ .x = 0, .y = 0, .width = 0, .height = 0 };

    pub fn contains(self: Rect, px: u16, py: u16) bool {
        return px >= self.x and px < self.x + self.width and
            py >= self.y and py < self.y + self.height;
    }

    pub fn intersect(self: Rect, other: Rect) ?Rect {
        const x1 = @max(self.x, other.x);
        const y1 = @max(self.y, other.y);
        const x2 = @min(self.x + self.width, other.x + other.width);
        const y2 = @min(self.y + self.height, other.y + other.height);
        if (x1 >= x2 or y1 >= y2) return null;
        return .{ .x = x1, .y = y1, .width = x2 - x1, .height = y2 - y1 };
    }

    pub fn shrink(self: Rect, top: u16, right: u16, bottom: u16, left: u16) Rect {
        const x = self.x + left;
        const y = self.y + top;
        const w = self.width -| left -| right;
        const h = self.height -| top -| bottom;
        return .{ .x = x, .y = y, .width = w, .height = h };
    }
};

pub const Direction = enum { horizontal, vertical };

pub const Align = enum { start, center, end };

pub const LayoutOpts = struct {
    direction: Direction = .vertical,
    padding_top: u16 = 0,
    padding_right: u16 = 0,
    padding_bottom: u16 = 0,
    padding_left: u16 = 0,
    gap: u16 = 0,
    alignment: Align = .start,
    justify: bool = false,
};

pub const ChildLayout = struct {
    /// Relative size: 0 = fit to content, 1+ = flex grow factor
    flex_grow: u16 = 0,
    /// If set, use this exact width instead of flex
    width: ?u16 = null,
    /// If set, use this exact height instead of flex
    height: ?u16 = null,
    /// Minimum width constraint
    min_width: u16 = 0,
    /// Minimum height constraint
    min_height: u16 = 0,
    /// Maximum width constraint
    max_width: u16 = 65535,
    /// Maximum height constraint
    max_height: u16 = 65535,
};

pub const LayoutResult = struct {
    rect: Rect,
    children: []Rect,
};

/// Calculate layout for a container with children
pub fn calculateLayout(
    allocator: std.mem.Allocator,
    container: Rect,
    opts: LayoutOpts,
    children: []const ChildLayout,
    child_sizes: []const Rect,
) !LayoutResult {
    const child_count = children.len;
    if (child_count == 0) {
        return .{ .rect = container, .children = &.{} };
    }

    const child_rects = try allocator.alloc(Rect, child_count);
    errdefer allocator.free(child_rects);

    const inner = container.shrink(opts.padding_top, opts.padding_right, opts.padding_bottom, opts.padding_left);

    switch (opts.direction) {
        .horizontal => {
            const total_gap = if (child_count > 1) opts.gap * @as(u16, @intCast(child_count - 1)) else 0;
            const available = inner.width -| total_gap;

            // First pass: calculate fixed sizes and total flex
            var total_flex: u16 = 0;
            var fixed_used: u16 = 0;
            for (children, 0..) |child, i| {
                if (child.width) |w| {
                    fixed_used += w;
                } else if (child.flex_grow > 0) {
                    total_flex += child.flex_grow;
                } else {
                    // Auto: use content size
                    const content_w = if (i < child_sizes.len) child_sizes[i].width else 0;
                    fixed_used += @max(child.min_width, @min(content_w, child.max_width));
                }
            }

            const flex_space = available -| fixed_used;
            var x = inner.x;

            for (children, 0..) |child, i| {
                const w: u16 = if (child.width) |cw|
                    cw
                else if (child.flex_grow > 0 and total_flex > 0)
                    @max(child.min_width, @min(child.max_width, flex_space * child.flex_grow / total_flex))
                else if (i < child_sizes.len)
                    @max(child.min_width, @min(child_sizes[i].width, child.max_width))
                else
                    child.min_width;

                const h: u16 = if (child.height) |ch|
                    ch
                else if (i < child_sizes.len)
                    @max(child.min_height, @min(child_sizes[i].height, inner.height, child.max_height))
                else
                    @max(child.min_height, @min(inner.height, child.max_height));

                child_rects[i] = .{ .x = x, .y = inner.y, .width = w, .height = h };
                x += w;
                if (i < child_count - 1) x += opts.gap;
            }
        },
        .vertical => {
            const total_gap = if (child_count > 1) opts.gap * @as(u16, @intCast(child_count - 1)) else 0;
            const available = inner.height -| total_gap;

            // First pass
            var total_flex: u16 = 0;
            var fixed_used: u16 = 0;
            for (children, 0..) |child, i| {
                if (child.height) |h| {
                    fixed_used += h;
                } else if (child.flex_grow > 0) {
                    total_flex += child.flex_grow;
                } else {
                    const content_h = if (i < child_sizes.len) child_sizes[i].height else 0;
                    fixed_used += @max(child.min_height, @min(content_h, child.max_height));
                }
            }

            const flex_space = available -| fixed_used;
            var y = inner.y;

            for (children, 0..) |child, i| {
                const w: u16 = if (child.width) |cw|
                    cw
                else if (i < child_sizes.len)
                    @max(child.min_width, @min(child_sizes[i].width, inner.width, child.max_width))
                else
                    @max(child.min_width, @min(inner.width, child.max_width));

                const h: u16 = if (child.height) |ch|
                    ch
                else if (child.flex_grow > 0 and total_flex > 0)
                    @max(child.min_height, @min(child.max_height, flex_space * child.flex_grow / total_flex))
                else if (i < child_sizes.len)
                    @max(child.min_height, @min(child_sizes[i].height, child.max_height))
                else
                    child.min_height;

                child_rects[i] = .{ .x = inner.x, .y = y, .width = w, .height = h };
                y += h;
                if (i < child_count - 1) y += opts.gap;
            }
        },
    }

    return .{ .rect = container, .children = child_rects };
}
