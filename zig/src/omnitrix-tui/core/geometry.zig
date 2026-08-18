//! omnitrix-tui: Core Geometry (Geometri ve Koordinat Katmanı)
//!
//! Özellikler:
//! - Rect: x, y, width, height, kesişim, birleşim, iç alan (inner), kırpma (clamp)
//! - Position: 2D imleç ve piksel konumu
//! - Margin ve Padding: Boşluk ve kenar payı hesaplamaları

const std = @import("std");

pub const Position = struct {
    x: u16 = 0,
    y: u16 = 0,

    pub fn eql(self: Position, other: Position) bool {
        return self.x == other.x and self.y == other.y;
    }
};

pub const Margin = struct {
    horizontal: u16 = 0,
    vertical: u16 = 0,

    pub fn all(val: u16) Margin {
        return .{ .horizontal = val, .vertical = val };
    }
};

pub const Padding = struct {
    left: u16 = 0,
    right: u16 = 0,
    top: u16 = 0,
    bottom: u16 = 0,

    pub fn all(val: u16) Padding {
        return .{ .left = val, .right = val, .top = val, .bottom = val };
    }

    pub fn horizontal(val: u16) Padding {
        return .{ .left = val, .right = val, .top = 0, .bottom = 0 };
    }

    pub fn vertical(val: u16) Padding {
        return .{ .left = 0, .right = 0, .top = val, .bottom = val };
    }
};

pub const Rect = struct {
    x: u16 = 0,
    y: u16 = 0,
    width: u16 = 0,
    height: u16 = 0,

    pub const zero: Rect = .{};

    pub fn init(x: u16, y: u16, width: u16, height: u16) Rect {
        return .{
            .x = x,
            .y = y,
            .width = width,
            .height = height,
        };
    }

    pub fn area(self: Rect) u32 {
        return @as(u32, self.width) * @as(u32, self.height);
    }

    pub fn isEmpty(self: Rect) bool {
        return self.width == 0 or self.height == 0;
    }

    pub fn left(self: Rect) u16 {
        return self.x;
    }

    pub fn right(self: Rect) u16 {
        return self.x + self.width;
    }

    pub fn top(self: Rect) u16 {
        return self.y;
    }

    pub fn bottom(self: Rect) u16 {
        return self.y + self.height;
    }

    pub fn contains(self: Rect, pos: Position) bool {
        return pos.x >= self.left() and pos.x < self.right() and pos.y >= self.top() and pos.y < self.bottom();
    }

    pub fn inner(self: Rect, margin: Margin) Rect {
        const double_h = margin.horizontal * 2;
        const double_v = margin.vertical * 2;

        if (self.width < double_h or self.height < double_v) {
            return Rect.zero;
        }

        return .{
            .x = self.x + margin.horizontal,
            .y = self.y + margin.vertical,
            .width = self.width - double_h,
            .height = self.height - double_v,
        };
    }

    pub fn innerWithPadding(self: Rect, pad: Padding) Rect {
        const total_w = pad.left + pad.right;
        const total_h = pad.top + pad.bottom;

        if (self.width < total_w or self.height < total_h) {
            return Rect.zero;
        }

        return .{
            .x = self.x + pad.left,
            .y = self.y + pad.top,
            .width = self.width - total_w,
            .height = self.height - total_h,
        };
    }

    pub fn intersection(self: Rect, other: Rect) Rect {
        const x1 = @max(self.left(), other.left());
        const y1 = @max(self.top(), other.top());
        const x2 = @min(self.right(), other.right());
        const y2 = @min(self.bottom(), other.bottom());

        if (x1 >= x2 or y1 >= y2) {
            return Rect.zero;
        }

        return .{
            .x = x1,
            .y = y1,
            .width = x2 - x1,
            .height = y2 - y1,
        };
    }

    pub fn unionRect(self: Rect, other: Rect) Rect {
        if (self.isEmpty()) return other;
        if (other.isEmpty()) return self;

        const x1 = @min(self.left(), other.left());
        const y1 = @min(self.top(), other.top());
        const x2 = @max(self.right(), other.right());
        const y2 = @max(self.bottom(), other.bottom());

        return .{
            .x = x1,
            .y = y1,
            .width = x2 - x1,
            .height = y2 - y1,
        };
    }

    pub fn clamp(self: Rect, other: Rect) Rect {
        return self.intersection(other);
    }

    pub fn eql(self: Rect, other: Rect) bool {
        return self.x == other.x and self.y == other.y and self.width == other.width and self.height == other.height;
    }
};

test "geometry rect hesaplamalari" {
    const r1 = Rect.init(0, 0, 100, 50);
    try std.testing.expectEqual(@as(u32, 5000), r1.area());
    try std.testing.expect(!r1.isEmpty());

    const inner_r = r1.inner(Margin.all(5));
    try std.testing.expectEqual(Rect.init(5, 5, 90, 40), inner_r);

    const r2 = Rect.init(50, 25, 100, 50);
    const inter = r1.intersection(r2);
    try std.testing.expectEqual(Rect.init(50, 25, 50, 25), inter);
}
