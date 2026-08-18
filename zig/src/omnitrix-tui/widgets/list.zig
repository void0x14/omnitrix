//! omnitrix-tui: Widget - List & Stateful List (Seçilebilir ve Kaydırılabilir Liste)
//!
//! Özellikler:
//! - ListItem: Metin, ikon ve stil
//! - ListState: Seçili öğe indeksi (selected), kaydırma konumu (offset)
//! - Vurgulama: Seçili satıra özel sembol (❯ ), stil ve arka plan
//! - Otomatik görünürlük (Scroll into view): Seçim hareket ettikçe kaydırma konumu güncellenir

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const block_mod = @import("block.zig");

pub const Style = cell_mod.Style;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Block = block_mod.Block;

pub const ListItem = struct {
    content: []const u8,
    style: Style = .default,
};

pub const ListState = struct {
    selected: ?usize = 0,
    offset: usize = 0,

    pub fn select(self: *ListState, index: ?usize) void {
        self.selected = index;
    }

    pub fn next(self: *ListState, total: usize) void {
        if (total == 0) return;
        if (self.selected) |sel| {
            if (sel + 1 < total) {
                self.selected = sel + 1;
            }
        } else {
            self.selected = 0;
        }
    }

    pub fn prev(self: *ListState) void {
        if (self.selected) |sel| {
            if (sel > 0) {
                self.selected = sel - 1;
            }
        }
    }
};

pub const List = struct {
    allocator: std.mem.Allocator,
    items: std.ArrayList(ListItem),
    block: ?Block = null,
    highlight_symbol: []const u8 = "❯ ",
    highlight_style: Style = .{ .fg = .bright_cyan, .modifier = .{ .bold = true } },

    pub fn init(allocator: std.mem.Allocator) List {
        return .{
            .allocator = allocator,
            .items = std.ArrayList(ListItem).empty,
            .block = null,
            .highlight_symbol = "❯ ",
            .highlight_style = .{ .fg = .bright_cyan, .modifier = .{ .bold = true } },
        };
    }

    pub fn deinit(self: *List) void {
        self.items.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addItem(self: *List, content: []const u8, style: Style) !void {
        try self.items.append(self.allocator, .{
            .content = content,
            .style = style,
        });
    }

    pub fn render(self: *const List, area: Rect, buf: *Buffer, state: *ListState) void {
        if (area.isEmpty() or self.items.items.len == 0) return;

        var render_area = area;
        if (self.block) |b| {
            b.render(area, buf);
            render_area = b.inner(area);
        }

        if (render_area.isEmpty()) return;

        const max_visible = render_area.height;
        const total = self.items.items.len;

        // Scroll offset ayarla
        if (state.selected) |sel| {
            if (sel >= state.offset + max_visible) {
                state.offset = sel - max_visible + 1;
            } else if (sel < state.offset) {
                state.offset = sel;
            }
        }

        var y = render_area.top();
        var i = state.offset;

        while (i < total and y < render_area.bottom()) : ({
            i += 1;
            y += 1;
        }) {
            const item = self.items.items[i];
            const is_selected = (state.selected != null and state.selected.? == i);

            var x = render_area.left();
            const max_w = render_area.width;

            if (is_selected) {
                const sym_w = buf.setString(x, y, self.highlight_symbol, self.highlight_style, max_w);
                x += sym_w;
                _ = buf.setString(x, y, item.content, self.highlight_style, if (max_w > sym_w) max_w - sym_w else 0);
            } else {
                // Boşluk bırakarak hizala
                const indent_w = buf.setString(x, y, "  ", item.style, max_w);
                x += indent_w;
                _ = buf.setString(x, y, item.content, item.style, if (max_w > indent_w) max_w - indent_w else 0);
            }
        }
    }
};

test "list render ve state navigasyon" {
    const area = Rect.init(0, 0, 40, 10);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var list = List.init(std.testing.allocator);
    defer list.deinit();

    try list.addItem("Item 1", .{ .fg = .white });
    try list.addItem("Item 2", .{ .fg = .white });
    try list.addItem("Item 3", .{ .fg = .white });

    var state = ListState{ .selected = 1, .offset = 0 };
    list.render(area, &buf, &state);

    // Item 2 seçili olmalı ve başında '❯' olmalı
    const c0 = buf.get(0, 1).?;
    try std.testing.expectEqualStrings("❯", c0.getSymbol());
}
