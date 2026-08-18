//! omnitrix-tui: Core 2D Buffer Diff & ANSI Serializer (Minimum Değişiklikli Render)
//!
//! Özellikler:
//! - İki Buffer'ı (Prev vs Curr) hücre bazında karşılaştırır.
//! - Sadece değişen hücreler için ANSI kaçış dizisi ve karakter üretir.
//! - İmleç konumunu (x, y) akıllı takip eder; bitişik hücrelerde `\x1b[H` atlaması yapmaz.
//! - Stil durumunu (fg, bg, modifier) takip eder; değişmeyen hücrelerde gereksiz ANSI kodu basmaz.
//! - Tüm kareyi tek bir atomik tamponda toplar (Zero-Flicker Single Syscall).

const std = @import("std");
const cell_mod = @import("cell.zig");
const buffer_mod = @import("buffer.zig");
const geom_mod = @import("geometry.zig");

pub const Cell = cell_mod.Cell;
pub const Style = cell_mod.Style;
pub const Color = cell_mod.Color;
pub const Modifier = cell_mod.Modifier;
pub const Buffer = buffer_mod.Buffer;
pub const Rect = geom_mod.Rect;

pub const BufferDiff = struct {
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator) BufferDiff {
        return .{ .allocator = allocator };
    }

    pub fn renderDiff(
        self: *const BufferDiff,
        prev: ?*const Buffer,
        curr: *const Buffer,
        out_buf: *std.ArrayList(u8),
    ) !void {
        _ = self;
        var last_x: ?u16 = null;
        var last_y: ?u16 = null;
        var last_style: Style = .{ .fg = .reset, .bg = .reset, .modifier = .empty };

        var y: u16 = curr.area.top();
        while (y < curr.area.bottom()) : (y += 1) {
            var x: u16 = curr.area.left();
            while (x < curr.area.right()) : (x += 1) {
                const cur_cell = curr.get(x, y) orelse continue;

                // Geniş karakterin (width == 0) ikinci yarısını atla
                if (cur_cell.width == 0) continue;

                // Önceki tamponla karşılaştır; aynı ise render etme
                if (prev) |p| {
                    if (p.get(x, y)) |prev_cell| {
                        if (cur_cell.eql(prev_cell)) {
                            continue;
                        }
                    }
                }

                // 1. İmleci konumlandır (Gerekiyorsa)
                if (last_x == null or last_y == null or last_y.? != y or last_x.? > x) {
                    var move_buf: [32]u8 = undefined;
                    const move_str = try std.fmt.bufPrint(&move_buf, "\x1b[{d};{d}H", .{ y + 1, x + 1 });
                    try out_buf.appendSlice(curr.allocator, move_str);
                } else if (last_x.? < x) {
                    const gap = x - last_x.?;
                    if (gap <= 4) {
                        var g: usize = 0;
                        while (g < gap) : (g += 1) {
                            try out_buf.appendSlice(curr.allocator, " ");
                        }
                    } else {
                        var move_buf: [32]u8 = undefined;
                        const move_str = try std.fmt.bufPrint(&move_buf, "\x1b[{d};{d}H", .{ y + 1, x + 1 });
                        try out_buf.appendSlice(curr.allocator, move_str);
                    }
                }

                // 2. Stili uygula (Gerekiyorsa)
                if (!cur_cell.style.eql(last_style)) {
                    try emitStyleDiff(out_buf, curr.allocator, last_style, cur_cell.style);
                    last_style = cur_cell.style;
                }

                // 3. Sembolü yaz
                try out_buf.appendSlice(curr.allocator, cur_cell.getSymbol());

                last_x = x + @as(u16, cur_cell.width);
                last_y = y;
            }
        }

        // Render bitiminde stili sıfırla
        if (!last_style.eql(.default)) {
            try out_buf.appendSlice(curr.allocator, "\x1b[0m");
        }
    }

    fn emitStyleDiff(
        out_buf: *std.ArrayList(u8),
        allocator: std.mem.Allocator,
        from: Style,
        to: Style,
    ) !void {
        _ = from;
        // Basit ve güvenli stil çıktısı
        try out_buf.appendSlice(allocator, "\x1b[0m"); // Reset

        if (to.modifier.bold) try out_buf.appendSlice(allocator, "\x1b[1m");
        if (to.modifier.dim) try out_buf.appendSlice(allocator, "\x1b[2m");
        if (to.modifier.italic) try out_buf.appendSlice(allocator, "\x1b[3m");
        if (to.modifier.underline) try out_buf.appendSlice(allocator, "\x1b[4m");
        if (to.modifier.reverse) try out_buf.appendSlice(allocator, "\x1b[7m");

        // FG Color
        switch (to.fg) {
            .reset => {},
            .black => try out_buf.appendSlice(allocator, "\x1b[30m"),
            .red => try out_buf.appendSlice(allocator, "\x1b[31m"),
            .green => try out_buf.appendSlice(allocator, "\x1b[32m"),
            .yellow => try out_buf.appendSlice(allocator, "\x1b[33m"),
            .blue => try out_buf.appendSlice(allocator, "\x1b[34m"),
            .magenta => try out_buf.appendSlice(allocator, "\x1b[35m"),
            .cyan => try out_buf.appendSlice(allocator, "\x1b[36m"),
            .white => try out_buf.appendSlice(allocator, "\x1b[37m"),
            .bright_black => try out_buf.appendSlice(allocator, "\x1b[90m"),
            .bright_red => try out_buf.appendSlice(allocator, "\x1b[91m"),
            .bright_green => try out_buf.appendSlice(allocator, "\x1b[92m"),
            .bright_yellow => try out_buf.appendSlice(allocator, "\x1b[93m"),
            .bright_blue => try out_buf.appendSlice(allocator, "\x1b[94m"),
            .bright_magenta => try out_buf.appendSlice(allocator, "\x1b[95m"),
            .bright_cyan => try out_buf.appendSlice(allocator, "\x1b[96m"),
            .bright_white => try out_buf.appendSlice(allocator, "\x1b[97m"),
            .indexed => |idx| {
                var tmp: [32]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[38;5;{d}m", .{idx});
                try out_buf.appendSlice(allocator, s);
            },
            .rgb => |rgb| {
                var tmp: [32]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[38;2;{d};{d};{d}m", .{ rgb.r, rgb.g, rgb.b });
                try out_buf.appendSlice(allocator, s);
            },
        }

        // BG Color
        switch (to.bg) {
            .reset => {},
            .black => try out_buf.appendSlice(allocator, "\x1b[40m"),
            .red => try out_buf.appendSlice(allocator, "\x1b[41m"),
            .green => try out_buf.appendSlice(allocator, "\x1b[42m"),
            .yellow => try out_buf.appendSlice(allocator, "\x1b[43m"),
            .blue => try out_buf.appendSlice(allocator, "\x1b[44m"),
            .magenta => try out_buf.appendSlice(allocator, "\x1b[45m"),
            .cyan => try out_buf.appendSlice(allocator, "\x1b[46m"),
            .white => try out_buf.appendSlice(allocator, "\x1b[47m"),
            .bright_black => try out_buf.appendSlice(allocator, "\x1b[100m"),
            .bright_red => try out_buf.appendSlice(allocator, "\x1b[101m"),
            .bright_green => try out_buf.appendSlice(allocator, "\x1b[102m"),
            .bright_yellow => try out_buf.appendSlice(allocator, "\x1b[103m"),
            .bright_blue => try out_buf.appendSlice(allocator, "\x1b[104m"),
            .bright_magenta => try out_buf.appendSlice(allocator, "\x1b[105m"),
            .bright_cyan => try out_buf.appendSlice(allocator, "\x1b[106m"),
            .bright_white => try out_buf.appendSlice(allocator, "\x1b[107m"),
            .indexed => |idx| {
                var tmp: [32]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[48;5;{d}m", .{idx});
                try out_buf.appendSlice(allocator, s);
            },
            .rgb => |rgb| {
                var tmp: [32]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[48;2;{d};{d};{d}m", .{ rgb.r, rgb.g, rgb.b });
                try out_buf.appendSlice(allocator, s);
            },
        }
    }
};

test "buffer diff render delta" {
    const area = Rect.init(0, 0, 80, 24);
    var prev = try Buffer.init(std.testing.allocator, area);
    defer prev.deinit();

    var curr = try Buffer.init(std.testing.allocator, area);
    defer curr.deinit();

    _ = curr.setString(5, 5, "Hello", .{ .fg = .green }, 80);

    const differ = BufferDiff.init(std.testing.allocator);
    var out = std.ArrayList(u8).empty;
    defer out.deinit(std.testing.allocator);

    try differ.renderDiff(&prev, &curr, &out);

    try std.testing.expect(out.items.len > 0);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\x1b[6;6H") != null); // row 5+1, col 5+1
    try std.testing.expect(std.mem.indexOf(u8, out.items, "Hello") != null);
}
