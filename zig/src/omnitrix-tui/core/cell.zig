//! omnitrix-tui: Core Terminal Cell (Hücre Katmanı)
//!
//! Her terminal hücresi:
//! - 1 adet Unicode karakteri (veya çok baytlı UTF-8 grapheme)
//! - Görsel sütun genişliği (1: normal ASCII/Latin, 2: CJK/Emoji, 0: birleştirici karakterler)
//! - Ön plan (fg) ve arka plan (bg) renkleri
//! - Metin biçimlendirme niteleyicileri (bold, dim, italic, underline, reverse, hidden, crossed_out)

const std = @import("std");

pub const Color = union(enum) {
    reset,
    black,
    red,
    green,
    yellow,
    blue,
    magenta,
    cyan,
    white,
    bright_black,
    bright_red,
    bright_green,
    bright_yellow,
    bright_blue,
    bright_magenta,
    bright_cyan,
    bright_white,
    indexed: u8,
    rgb: struct { r: u8, g: u8, b: u8 },

    pub fn eql(self: Color, other: Color) bool {
        const Tag = std.meta.Tag(Color);
        if (@as(Tag, self) != @as(Tag, other)) return false;
        return switch (self) {
            .indexed => |i| i == other.indexed,
            .rgb => |rgb| rgb.r == other.rgb.r and rgb.g == other.rgb.g and rgb.b == other.rgb.b,
            else => true,
        };
    }
};

pub const Modifier = packed struct {
    bold: bool = false,
    dim: bool = false,
    italic: bool = false,
    underline: bool = false,
    reverse: bool = false,
    hidden: bool = false,
    crossed_out: bool = false,
    _padding: u1 = 0,

    pub const empty: Modifier = .{};

    pub fn contains(self: Modifier, other: Modifier) bool {
        if (other.bold and !self.bold) return false;
        if (other.dim and !self.dim) return false;
        if (other.italic and !self.italic) return false;
        if (other.underline and !self.underline) return false;
        if (other.reverse and !self.reverse) return false;
        if (other.hidden and !self.hidden) return false;
        if (other.crossed_out and !self.crossed_out) return false;
        return true;
    }

    pub fn eql(self: Modifier, other: Modifier) bool {
        return @as(u8, @bitCast(self)) == @as(u8, @bitCast(other));
    }
};

pub const Style = struct {
    fg: Color = .reset,
    bg: Color = .reset,
    modifier: Modifier = .empty,

    pub const default: Style = .{};

    pub fn fgColor(color: Color) Style {
        return .{ .fg = color };
    }

    pub fn bgColor(color: Color) Style {
        return .{ .bg = color };
    }

    pub fn withBold(self: Style) Style {
        var res = self;
        res.modifier.bold = true;
        return res;
    }

    pub fn withDim(self: Style) Style {
        var res = self;
        res.modifier.dim = true;
        return res;
    }

    pub fn withItalic(self: Style) Style {
        var res = self;
        res.modifier.italic = true;
        return res;
    }

    pub fn withUnderline(self: Style) Style {
        var res = self;
        res.modifier.underline = true;
        return res;
    }

    pub fn withReverse(self: Style) Style {
        var res = self;
        res.modifier.reverse = true;
        return res;
    }

    pub fn eql(self: Style, other: Style) bool {
        return self.fg.eql(other.fg) and self.bg.eql(other.bg) and self.modifier.eql(other.modifier);
    }
};

pub const Cell = struct {
    symbol: [8]u8 = [_]u8{ ' ', 0, 0, 0, 0, 0, 0, 0 },
    symbol_len: u4 = 1,
    width: u2 = 1,
    style: Style = .default,

    pub const default: Cell = .{};

    pub fn init(char: u8, style: Style) Cell {
        var c = Cell{
            .symbol_len = 1,
            .width = 1,
            .style = style,
        };
        c.symbol[0] = char;
        return c;
    }

    pub fn initUtf8(bytes: []const u8, width: u2, style: Style) Cell {
        var c = Cell{
            .symbol_len = @intCast(@min(bytes.len, 8)),
            .width = width,
            .style = style,
        };
        const copy_len = @min(bytes.len, 8);
        @memcpy(c.symbol[0..copy_len], bytes[0..copy_len]);
        return c;
    }

    pub fn reset(self: *Cell) void {
        self.symbol = [_]u8{ ' ', 0, 0, 0, 0, 0, 0, 0 };
        self.symbol_len = 1;
        self.width = 1;
        self.style = .default;
    }

    pub fn getSymbol(self: *const Cell) []const u8 {
        return self.symbol[0..self.symbol_len];
    }

    pub fn setSymbol(self: *Cell, bytes: []const u8, width: u2) void {
        const copy_len: usize = @min(bytes.len, 8);
        @memcpy(self.symbol[0..copy_len], bytes[0..copy_len]);
        self.symbol_len = @intCast(copy_len);
        self.width = width;
    }

    pub fn setChar(self: *Cell, char: u8) void {
        self.symbol[0] = char;
        self.symbol_len = 1;
        self.width = 1;
    }

    pub fn setStyle(self: *Cell, style: Style) void {
        self.style = style;
    }

    pub fn eql(self: Cell, other: Cell) bool {
        if (self.symbol_len != other.symbol_len) return false;
        if (self.width != other.width) return false;
        if (!self.style.eql(other.style)) return false;
        return std.mem.eql(u8, self.getSymbol(), other.getSymbol());
    }
};

test "cell init ve degisim" {
    var c = Cell.init('A', .{ .fg = .green });
    try std.testing.expectEqualStrings("A", c.getSymbol());
    try std.testing.expectEqual(@as(u2, 1), c.width);
    try std.testing.expect(c.style.fg.eql(.green));

    c.setSymbol("🚀", 2);
    try std.testing.expectEqualStrings("🚀", c.getSymbol());
    try std.testing.expectEqual(@as(u2, 2), c.width);

    c.reset();
    try std.testing.expectEqualStrings(" ", c.getSymbol());
}
