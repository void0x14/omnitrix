//! omnitrix-tui: Widget - Tabs (Sekme Çubuğu)
//!
//! Özellikler:
//! - Çoklu sekme başlıkları
//! - Aktif sekme vurgusu (arka plan, renk ve kalın yazı)
//! - Ayırıcı çizgiler (│ veya /)
//! - Güvenli hücre çizimi

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const block_mod = @import("block.zig");

pub const Style = cell_mod.Style;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Block = block_mod.Block;

pub const Tabs = struct {
    allocator: std.mem.Allocator,
    titles: std.ArrayList([]const u8),
    selected: usize = 0,
    block: ?Block = null,
    style: Style = .{ .fg = .{ .indexed = 244 } },
    highlight_style: Style = .{ .fg = .bright_white, .bg = .{ .indexed = 239 }, .modifier = .{ .bold = true } },
    divider: []const u8 = "│",

    pub fn init(allocator: std.mem.Allocator) Tabs {
        return .{
            .allocator = allocator,
            .titles = std.ArrayList([]const u8).empty,
            .selected = 0,
            .block = null,
            .style = .{ .fg = .{ .indexed = 244 } },
            .highlight_style = .{ .fg = .bright_white, .bg = .{ .indexed = 239 }, .modifier = .{ .bold = true } },
            .divider = "│",
        };
    }

    pub fn deinit(self: *Tabs) void {
        self.titles.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addTab(self: *Tabs, title: []const u8) !void {
        try self.titles.append(self.allocator, title);
    }

    pub fn render(self: *const Tabs, area: Rect, buf: *Buffer) void {
        if (area.isEmpty() or self.titles.items.len == 0) return;

        var render_area = area;
        if (self.block) |b| {
            b.render(area, buf);
            render_area = b.inner(area);
        }

        if (render_area.isEmpty()) return;

        var x = render_area.left();
        const y = render_area.top();

        for (self.titles.items, 0..) |title, idx| {
            if (x >= render_area.right()) break;

            const is_selected = (idx == self.selected);
            const st = if (is_selected) self.highlight_style else self.style;

            // Boşluk
            x += buf.setString(x, y, " ", st, render_area.right() - x);

            // Başlık
            const title_w = buf.setString(x, y, title, st, render_area.right() - x);
            x += title_w;

            // Boşluk
            x += buf.setString(x, y, " ", st, render_area.right() - x);

            // Ayırıcı
            if (idx + 1 < self.titles.items.len and x < render_area.right()) {
                x += buf.setString(x, y, self.divider, self.style, render_area.right() - x);
            }
        }
    }
};

test "tabs render ve secim" {
    const area = Rect.init(0, 0, 60, 1);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var tabs = Tabs.init(std.testing.allocator);
    defer tabs.deinit();

    try tabs.addTab("Tab 1");
    try tabs.addTab("Tab 2");
    tabs.selected = 0;

    tabs.render(area, &buf);

    const c1 = buf.get(1, 0).?;
    try std.testing.expectEqualStrings("T", c1.getSymbol());
    try std.testing.expect(c1.style.fg.eql(.bright_white));
}
