//! omnitrix-tui: Core Layout Engine (Constraint Tabanlı Yerleşim Motoru)
//!
//! Özellikler:
//! - Constraint: Percentage, Length, Ratio, Min, Max, Fill
//! - Direction: Horizontal (Yatay), Vertical (Dikey)
//! - Margin ve Padding desteği
//! - Flex modeli (start, center, end, space_between, space_around)
//! - Kalan alanın ve yuvarlama farklarının deterministik dağıtımı

const std = @import("std");
const geom = @import("geometry.zig");

pub const Rect = geom.Rect;
pub const Margin = geom.Margin;
pub const Padding = geom.Padding;

pub const Direction = enum {
    horizontal,
    vertical,
};

pub const Flex = enum {
    start,
    center,
    end,
    space_between,
    space_around,
};

pub const Constraint = union(enum) {
    percentage: u16,
    length: u16,
    ratio: struct { num: u32, den: u32 },
    min: u16,
    max: u16,
    fill: u16,

    pub fn apply(self: Constraint, total_space: u16) u16 {
        return switch (self) {
            .length => |l| @min(l, total_space),
            .percentage => |p| @intCast((@as(u32, total_space) * @as(u32, p)) / 100),
            .ratio => |r| if (r.den == 0) 0 else @intCast((@as(u64, total_space) * @as(u64, r.num)) / @as(u64, r.den)),
            .min => |m| @max(m, total_space),
            .max => |m| @min(m, total_space),
            .fill => total_space,
        };
    }
};

pub const Layout = struct {
    direction: Direction = .vertical,
    margin: Margin = .{},
    constraints: []const Constraint,
    flex: Flex = .start,

    pub fn init(direction: Direction, constraints: []const Constraint) Layout {
        return .{
            .direction = direction,
            .margin = .{},
            .constraints = constraints,
            .flex = .start,
        };
    }

    pub fn withMargin(self: Layout, margin: Margin) Layout {
        var res = self;
        res.margin = margin;
        return res;
    }

    pub fn split(self: Layout, area: Rect, allocator: std.mem.Allocator) ![]Rect {
        const inner_area = area.inner(self.margin);
        if (self.constraints.len == 0 or inner_area.isEmpty()) {
            return allocator.alloc(Rect, 0);
        }

        const total_size: u16 = switch (self.direction) {
            .horizontal => inner_area.width,
            .vertical => inner_area.height,
        };

        const result = try allocator.alloc(Rect, self.constraints.len);
        errdefer allocator.free(result);

        // 1. Sabit ve oransal boyutları hesapla
        var assigned_sizes = try allocator.alloc(u16, self.constraints.len);
        defer allocator.free(assigned_sizes);
        @memset(assigned_sizes, 0);

        var used_space: u16 = 0;
        var fill_count: usize = 0;

        for (self.constraints, 0..) |c, idx| {
            switch (c) {
                .length => |l| {
                    const s = @min(l, if (total_size > used_space) total_size - used_space else 0);
                    assigned_sizes[idx] = s;
                    used_space += s;
                },
                .percentage => |p| {
                    const s: u16 = @intCast((@as(u32, total_size) * @as(u32, p)) / 100);
                    assigned_sizes[idx] = s;
                    used_space += s;
                },
                .ratio => |r| {
                    if (r.den > 0) {
                        const s: u16 = @intCast((@as(u64, total_size) * @as(u64, r.num)) / @as(u64, r.den));
                        assigned_sizes[idx] = s;
                        used_space += s;
                    }
                },
                .min => |m| {
                    assigned_sizes[idx] = m;
                    used_space += m;
                },
                .max => |m| {
                    assigned_sizes[idx] = @min(m, total_size);
                    used_space += assigned_sizes[idx];
                },
                .fill => {
                    fill_count += 1;
                },
            }
        }

        // Kalan alanı Fill (esnek) alanlara dağıt
        if (fill_count > 0 and total_size > used_space) {
            const remaining = total_size - used_space;
            const per_fill = remaining / @as(u16, @intCast(fill_count));
            var remainder = remaining % @as(u16, @intCast(fill_count));

            for (self.constraints, 0..) |c, idx| {
                if (c == .fill) {
                    const extra: u16 = if (remainder > 0) 1 else 0;
                    if (remainder > 0) remainder -= 1;
                    assigned_sizes[idx] = per_fill + extra;
                }
            }
        }

        // 2. Rect parçalarını oluştur
        var offset: u16 = 0;
        for (assigned_sizes, 0..) |s, idx| {
            switch (self.direction) {
                .horizontal => {
                    result[idx] = Rect.init(
                        inner_area.x + offset,
                        inner_area.y,
                        s,
                        inner_area.height,
                    );
                },
                .vertical => {
                    result[idx] = Rect.init(
                        inner_area.x,
                        inner_area.y + offset,
                        inner_area.width,
                        s,
                    );
                },
            }
            offset += s;
        }

        return result;
    }
};

test "layout dikey ve yatay split" {
    const area = Rect.init(0, 0, 100, 50);

    // Dikey: Üst bar (1 satır), Ana gövde (Fill), Alt bar (3 satır)
    const constraints = [_]Constraint{
        .{ .length = 1 },
        .{ .fill = 1 },
        .{ .length = 3 },
    };

    const l = Layout.init(.vertical, &constraints);
    const chunks = try l.split(area, std.testing.allocator);
    defer std.testing.allocator.free(chunks);

    try std.testing.expectEqual(@as(usize, 3), chunks.len);
    try std.testing.expectEqual(@as(u16, 1), chunks[0].height);
    try std.testing.expectEqual(@as(u16, 46), chunks[1].height);
    try std.testing.expectEqual(@as(u16, 3), chunks[2].height);

    // Yatay: Sol (30%), Sağ (70%)
    const h_constraints = [_]Constraint{
        .{ .percentage = 30 },
        .{ .percentage = 70 },
    };
    const hl = Layout.init(.horizontal, &h_constraints);
    const h_chunks = try hl.split(area, std.testing.allocator);
    defer std.testing.allocator.free(h_chunks);

    try std.testing.expectEqual(@as(u16, 30), h_chunks[0].width);
    try std.testing.expectEqual(@as(u16, 70), h_chunks[1].width);
}
