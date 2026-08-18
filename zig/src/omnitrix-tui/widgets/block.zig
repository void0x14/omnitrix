//! omnitrix-tui: Widget - Block (Kenarlık, Başlık ve Arka Plan Çerçevesi)
//!
//! Özellikler:
//! - Kenarlık Türleri: Rounded (╭╮╰╯), Plain (┌┐└┘), Double (╔╗╚╝), Thick (┏┓┗┛), None
//! - Kenarlık Yönleri: Tümü, Üst, Alt, Sol, Sağ
//! - Başlık Konumlandırma: Üst-Sol, Üst-Orta, Üst-Sağ, Alt-Sol, Alt-Sağ
//! - İç Alan Hesaplama (inner): Çocuk widget'lar için güvenli kırpılmış alan döner

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");

pub const Style = cell_mod.Style;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;

pub const BorderType = enum {
    none,
    plain,
    rounded,
    double,
    thick,
};

pub const BorderSides = packed struct {
    top: bool = true,
    bottom: bool = true,
    left: bool = true,
    right: bool = true,

    pub const all: BorderSides = .{ .top = true, .bottom = true, .left = true, .right = true };
    pub const none: BorderSides = .{ .top = false, .bottom = false, .left = false, .right = false };
    pub const left_only: BorderSides = .{ .top = false, .bottom = false, .left = true, .right = false };
};

pub const TitleAlignment = enum {
    left,
    center,
    right,
};

pub const TitlePosition = enum {
    top,
    bottom,
};

pub const Title = struct {
    content: []const u8,
    style: Style = .default,
    alignment: TitleAlignment = .left,
    position: TitlePosition = .top,
};

pub const BorderSymbols = struct {
    top_left: []const u8,
    top_right: []const u8,
    bottom_left: []const u8,
    bottom_right: []const u8,
    horizontal: []const u8,
    vertical: []const u8,

    pub fn get(border_type: BorderType) BorderSymbols {
        return switch (border_type) {
            .none => .{
                .top_left = " ",
                .top_right = " ",
                .bottom_left = " ",
                .bottom_right = " ",
                .horizontal = " ",
                .vertical = " ",
            },
            .plain => .{
                .top_left = "┌",
                .top_right = "┐",
                .bottom_left = "└",
                .bottom_right = "┘",
                .horizontal = "─",
                .vertical = "│",
            },
            .rounded => .{
                .top_left = "╭",
                .top_right = "╮",
                .bottom_left = "╰",
                .bottom_right = "╯",
                .horizontal = "─",
                .vertical = "│",
            },
            .double => .{
                .top_left = "╔",
                .top_right = "╗",
                .bottom_left = "╚",
                .bottom_right = "╝",
                .horizontal = "═",
                .vertical = "║",
            },
            .thick => .{
                .top_left = "┏",
                .top_right = "┓",
                .bottom_left = "┗",
                .bottom_right = "┛",
                .horizontal = "━",
                .vertical = "┃",
            },
        };
    }
};

pub const Block = struct {
    border_type: BorderType = .rounded,
    border_sides: BorderSides = .all,
    border_style: Style = .default,
    bg_style: Style = .default,
    titles: [4]?Title = .{ null, null, null, null },
    title_count: usize = 0,

    pub fn init() Block {
        return .{};
    }

    pub fn addTitle(self: *Block, content: []const u8, style: Style, align_mode: TitleAlignment, pos: TitlePosition) void {
        if (self.title_count < 4) {
            self.titles[self.title_count] = .{
                .content = content,
                .style = style,
                .alignment = align_mode,
                .position = pos,
            };
            self.title_count += 1;
        }
    }

    pub fn inner(self: *const Block, area: Rect) Rect {
        var res = area;
        if (self.border_sides.left and res.width > 0) {
            res.x += 1;
            res.width -= 1;
        }
        if (self.border_sides.right and res.width > 0) {
            res.width -= 1;
        }
        if (self.border_sides.top and res.height > 0) {
            res.y += 1;
            res.height -= 1;
        }
        if (self.border_sides.bottom and res.height > 0) {
            res.height -= 1;
        }
        return res;
    }

    pub fn render(self: *const Block, area: Rect, buf: *Buffer) void {
        if (area.isEmpty()) return;

        // 1. Arka plan stilini uygula
        if (!self.bg_style.eql(.default)) {
            buf.setStyle(area, self.bg_style);
        }

        if (self.border_type == .none and self.border_sides.left == false and self.border_sides.right == false and self.border_sides.top == false and self.border_sides.bottom == false) {
            return;
        }

        const syms = BorderSymbols.get(self.border_type);
        const top = area.top();
        const bottom = area.bottom() - 1;
        const left = area.left();
        const right = area.right() - 1;

        // 2. Kenarlık Çizgileri
        // Üst kenar
        if (self.border_sides.top) {
            var x = left;
            while (x <= right) : (x += 1) {
                if (buf.getMut(x, top)) |c| {
                    c.setSymbol(syms.horizontal, 1);
                    c.setStyle(self.border_style);
                }
            }
        }

        // Alt kenar
        if (self.border_sides.bottom and bottom > top) {
            var x = left;
            while (x <= right) : (x += 1) {
                if (buf.getMut(x, bottom)) |c| {
                    c.setSymbol(syms.horizontal, 1);
                    c.setStyle(self.border_style);
                }
            }
        }

        // Sol kenar
        if (self.border_sides.left) {
            var y = top;
            while (y <= bottom) : (y += 1) {
                if (buf.getMut(left, y)) |c| {
                    c.setSymbol(syms.vertical, 1);
                    c.setStyle(self.border_style);
                }
            }
        }

        // Sağ kenar
        if (self.border_sides.right and right > left) {
            var y = top;
            while (y <= bottom) : (y += 1) {
                if (buf.getMut(right, y)) |c| {
                    c.setSymbol(syms.vertical, 1);
                    c.setStyle(self.border_style);
                }
            }
        }

        // 3. Köşeler
        if (self.border_sides.top and self.border_sides.left) {
            if (buf.getMut(left, top)) |c| {
                c.setSymbol(syms.top_left, 1);
                c.setStyle(self.border_style);
            }
        }
        if (self.border_sides.top and self.border_sides.right and right > left) {
            if (buf.getMut(right, top)) |c| {
                c.setSymbol(syms.top_right, 1);
                c.setStyle(self.border_style);
            }
        }
        if (self.border_sides.bottom and self.border_sides.left and bottom > top) {
            if (buf.getMut(left, bottom)) |c| {
                c.setSymbol(syms.bottom_left, 1);
                c.setStyle(self.border_style);
            }
        }
        if (self.border_sides.bottom and self.border_sides.right and bottom > top and right > left) {
            if (buf.getMut(right, bottom)) |c| {
                c.setSymbol(syms.bottom_right, 1);
                c.setStyle(self.border_style);
            }
        }

        // 4. Başlıklar
        var i: usize = 0;
        while (i < self.title_count) : (i += 1) {
            if (self.titles[i]) |title| {
                const target_y = if (title.position == .top) top else bottom;
                const max_title_w = if (area.width > 4) area.width - 4 else area.width;

                var title_x = left + 2;
                if (title.alignment == .right and area.width > title.content.len + 3) {
                    title_x = right - 1 - @as(u16, @intCast(title.content.len));
                } else if (title.alignment == .center and area.width > title.content.len + 2) {
                    title_x = left + (area.width - @as(u16, @intCast(title.content.len))) / 2;
                }

                _ = buf.setString(title_x, target_y, title.content, title.style, max_title_w);
            }
        }
    }
};

test "block render ve inner hesaplama" {
    const area = Rect.init(0, 0, 40, 10);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var block = Block.init();
    block.border_type = .rounded;
    block.addTitle("Omnitrix", .{ .fg = .cyan }, .left, .top);

    const inside = block.inner(area);
    try std.testing.expectEqual(Rect.init(1, 1, 38, 8), inside);

    block.render(area, &buf);

    const tl = buf.get(0, 0).?;
    try std.testing.expectEqualStrings("╭", tl.getSymbol());

    const title_c = buf.get(2, 0).?;
    try std.testing.expectEqualStrings("O", title_c.getSymbol());
    try std.testing.expect(title_c.style.fg.eql(.cyan));
}
