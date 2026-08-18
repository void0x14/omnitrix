const std = @import("std");
const unicode_helper = @import("unicode.zig");

pub const Color = union(enum) {
    index: u8,
    rgb: RGB,
    named: NamedColor,

    pub const RGB = struct {
        r: u8,
        g: u8,
        b: u8,
    };

    pub const NamedColor = enum {
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
        default_bg,
        default_fg,
    };

    pub fn toAnsiFg(self: Color, buf: *[32]u8) []const u8 {
        return switch (self) {
            .index => |i| std.fmt.bufPrint(buf, "\x1b[38;5;{d}m", .{i}) catch "\x1b[39m",
            .rgb => |rgb| std.fmt.bufPrint(buf, "\x1b[38;2;{d};{d};{d}m", .{ rgb.r, rgb.g, rgb.b }) catch "\x1b[39m",
            .named => |nc| switch (nc) {
                .black => "\x1b[30m", .red => "\x1b[31m", .green => "\x1b[32m",
                .yellow => "\x1b[33m", .blue => "\x1b[34m", .magenta => "\x1b[35m",
                .cyan => "\x1b[36m", .white => "\x1b[37m", .bright_black => "\x1b[90m",
                .bright_red => "\x1b[91m", .bright_green => "\x1b[92m", .bright_yellow => "\x1b[93m",
                .bright_blue => "\x1b[94m", .bright_magenta => "\x1b[95m", .bright_cyan => "\x1b[96m",
                .bright_white => "\x1b[97m", .default_bg => "\x1b[49m", .default_fg => "\x1b[39m",
            },
        };
    }

    pub fn toAnsiBg(self: Color, buf: *[32]u8) []const u8 {
        return switch (self) {
            .index => |i| std.fmt.bufPrint(buf, "\x1b[48;5;{d}m", .{i}) catch "\x1b[49m",
            .rgb => |rgb| std.fmt.bufPrint(buf, "\x1b[48;2;{d};{d};{d}m", .{ rgb.r, rgb.g, rgb.b }) catch "\x1b[49m",
            .named => |nc| switch (nc) {
                .black => "\x1b[40m", .red => "\x1b[41m", .green => "\x1b[42m",
                .yellow => "\x1b[43m", .blue => "\x1b[44m", .magenta => "\x1b[45m",
                .cyan => "\x1b[46m", .white => "\x1b[47m", .bright_black => "\x1b[100m",
                .bright_red => "\x1b[101m", .bright_green => "\x1b[102m", .bright_yellow => "\x1b[103m",
                .bright_blue => "\x1b[104m", .bright_magenta => "\x1b[105m", .bright_cyan => "\x1b[106m",
                .bright_white => "\x1b[107m", .default_bg => "\x1b[49m", .default_fg => "\x1b[39m",
            },
        };
    }

    pub fn eq(self: Color, other: Color) bool {
        return std.meta.eql(self, other);
    }
};

pub const Attributes = packed struct {
    bold: bool = false,
    dim: bool = false,
    italic: bool = false,
    underline: bool = false,
    blink: bool = false,
    reverse: bool = false,
    strikethrough: bool = false,
    _padding: u9 = 0,

    pub fn toAnsi(self: Attributes, buf: *[32]u8) []const u8 {
        var parts: [8][]const u8 = undefined;
        var count: usize = 0;
        if (self.bold) { parts[count] = "1"; count += 1; }
        if (self.dim) { parts[count] = "2"; count += 1; }
        if (self.italic) { parts[count] = "3"; count += 1; }
        if (self.underline) { parts[count] = "4"; count += 1; }
        if (self.blink) { parts[count] = "5"; count += 1; }
        if (self.reverse) { parts[count] = "7"; count += 1; }
        if (self.strikethrough) { parts[count] = "9"; count += 1; }
        if (count == 0) return "\x1b[0m";
        var pos: usize = 0;
        buf[pos] = '\x1b'; pos += 1;
        buf[pos] = '['; pos += 1;
        for (parts[0..count], 0..) |part, i| {
            for (part) |ch| { buf[pos] = ch; pos += 1; }
            if (i < count - 1) { buf[pos] = ';'; pos += 1; }
        }
        buf[pos] = 'm'; pos += 1;
        return buf[0..pos];
    }

    pub fn eq(self: Attributes, other: Attributes) bool {
        return @as(u16, @bitCast(self)) == @as(u16, @bitCast(other));
    }
};

pub const Style = struct {
    fg: Color = .{ .named = .default_fg },
    bg: Color = .{ .named = .default_bg },
    attr: Attributes = .{},
    pub const default: Style = .{};
    pub fn eq(self: Style, other: Style) bool {
        return self.fg.eq(other.fg) and self.bg.eq(other.bg) and self.attr.eq(other.attr);
    }
};

pub const Cell = struct {
    char: union(enum) {
        empty,
        char: u21,
        wide_left,
        wide_right,
    } = .empty,
    style: Style = Style.default,
    width: u8 = 1,

    pub fn isEmpty(self: Cell) bool {
        return switch (self.char) { .empty => true, else => false };
    }

    pub fn eq(self: Cell, other: Cell) bool {
        if (!self.style.eq(other.style)) return false;
        if (self.width != other.width) return false;
        return switch (self.char) {
            .empty => switch (other.char) { .empty => true, else => false },
            .char => |c| switch (other.char) { .char => |oc| c == oc, else => false },
            .wide_left => other.char == .wide_left,
            .wide_right => other.char == .wide_right,
        };
    }

    pub fn writeUtf8(self: Cell, buf: *[4]u8) []const u8 {
        return switch (self.char) {
            .empty => " ",
            .char => |c| unicode_helper.encodeCodepoint(c, buf),
            .wide_left, .wide_right => " ",
        };
    }
};

test "Cell defaults" {
    const cell = Cell{};
    try std.testing.expect(cell.isEmpty());
    try std.testing.expect(cell.width == 1);
}
