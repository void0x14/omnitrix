//! omnitrix-tui: Core 2D Terminal Buffer (Tampon Matrisi)
//!
//! Özellikler:
//! - 2 Boyutlu hücre matrisi (width x height)
//! - Güvenli indeksleme, sınır denetimi ve kırpma (clipping)
//! - UTF-8 metin yazma, geniş karakter (wide character / CJK / Emoji) desteği
//! - Bölgesel stil uygulama (setStyle) ve tampon temizleme (clear)

const std = @import("std");
const cell_mod = @import("cell.zig");
const geom_mod = @import("geometry.zig");
const unicode_mod = @import("../unicode.zig");

pub const Cell = cell_mod.Cell;
pub const Style = cell_mod.Style;
pub const Rect = geom_mod.Rect;
pub const Position = geom_mod.Position;

pub const Buffer = struct {
    allocator: std.mem.Allocator,
    area: Rect,
    content: []Cell,

    pub fn init(allocator: std.mem.Allocator, area: Rect) !Buffer {
        const total = area.area();
        const content = try allocator.alloc(Cell, total);
        @memset(content, Cell.default);

        return .{
            .allocator = allocator,
            .area = area,
            .content = content,
        };
    }

    pub fn deinit(self: *Buffer) void {
        self.allocator.free(self.content);
        self.* = undefined;
    }

    pub fn index(self: *const Buffer, x: u16, y: u16) ?usize {
        if (x < self.area.left() or x >= self.area.right()) return null;
        if (y < self.area.top() or y >= self.area.bottom()) return null;

        const rel_x = x - self.area.x;
        const rel_y = y - self.area.y;
        return @as(usize, rel_y) * @as(usize, self.area.width) + @as(usize, rel_x);
    }

    pub fn get(self: *const Buffer, x: u16, y: u16) ?Cell {
        const idx = self.index(x, y) orelse return null;
        return self.content[idx];
    }

    pub fn getMut(self: *Buffer, x: u16, y: u16) ?*Cell {
        const idx = self.index(x, y) orelse return null;
        return &self.content[idx];
    }

    pub fn set(self: *Buffer, x: u16, y: u16, c: Cell) void {
        const ptr = self.getMut(x, y) orelse return;
        ptr.* = c;
    }

    pub fn clear(self: *Buffer) void {
        @memset(self.content, Cell.default);
    }

    pub fn resize(self: *Buffer, new_area: Rect) !void {
        if (self.area.eql(new_area)) return;

        const total = new_area.area();
        const new_content = try self.allocator.alloc(Cell, total);
        @memset(new_content, Cell.default);

        self.allocator.free(self.content);
        self.area = new_area;
        self.content = new_content;
    }

    /// Tampona UTF-8 metin yazar.
    /// Döner: Yazılan toplam görsel sütun genişliği
    pub fn setString(
        self: *Buffer,
        start_x: u16,
        start_y: u16,
        string: []const u8,
        style: Style,
        max_width: u16,
    ) u16 {
        if (start_y < self.area.top() or start_y >= self.area.bottom()) return 0;
        if (start_x >= self.area.right()) return 0;

        var cur_x = start_x;
        var written_width: u16 = 0;
        var i: usize = 0;

        while (i < string.len and written_width < max_width and cur_x < self.area.right()) {
            const byte = string[i];

            // ANSI kaçış dizilerini doğrudan hücreye yazma (atla veya stil olarak uygula)
            if (byte == '\x1b') {
                while (i < string.len and string[i] != 'm') : (i += 1) {}
                if (i < string.len) i += 1;
                continue;
            }

            // UTF-8 kod noktasını çözümle
            const seq_len = std.unicode.utf8ByteSequenceLength(byte) catch 1;
            if (i + seq_len > string.len) break;

            const char_slice = string[i .. i + seq_len];
            const cp = std.unicode.utf8Decode(char_slice) catch ' ';
            const char_w: u2 = @intCast(@min(unicode_mod.codepointWidth(cp), 2));

            if (char_w == 0) {
                // Sıfır genişlikli birleştirici karakter
                i += seq_len;
                continue;
            }

            if (written_width + char_w > max_width or cur_x + char_w > self.area.right()) {
                break;
            }

            if (self.getMut(cur_x, start_y)) |c| {
                c.setSymbol(char_slice, char_w);
                c.setStyle(style);
            }

            // Geniş karakter (CJK / Emoji) ise sağındaki hücreyi boş bırak
            if (char_w == 2 and cur_x + 1 < self.area.right()) {
                if (self.getMut(cur_x + 1, start_y)) |next_c| {
                    next_c.setSymbol("", 0);
                    next_c.setStyle(style);
                }
            }

            cur_x += char_w;
            written_width += char_w;
            i += seq_len;
        }

        return written_width;
    }

    /// Belirtilen dikdörtgen alana stil uygular.
    pub fn setStyle(self: *Buffer, rect: Rect, style: Style) void {
        const clipped = self.area.intersection(rect);
        if (clipped.isEmpty()) return;

        var y = clipped.top();
        while (y < clipped.bottom()) : (y += 1) {
            var x = clipped.left();
            while (x < clipped.right()) : (x += 1) {
                if (self.getMut(x, y)) |c| {
                    c.setStyle(style);
                }
            }
        }
    }

    /// Tamponun herhangi bir satırında belirtilen metnin geçip geçmediğini kontrol eder.
    pub fn containsText(self: *const Buffer, needle: []const u8) bool {
        if (needle.len == 0 or self.area.isEmpty()) return true;

        var y: u16 = self.area.top();
        while (y < self.area.bottom()) : (y += 1) {
            var row_bytes: [512]u8 = undefined;
            var pos: usize = 0;

            var x: u16 = self.area.left();
            while (x < self.area.right()) : (x += 1) {
                if (self.get(x, y)) |c| {
                    const sym = c.getSymbol();
                    if (pos + sym.len <= row_bytes.len) {
                        @memcpy(row_bytes[pos .. pos + sym.len], sym);
                        pos += sym.len;
                    }
                }
            }

            if (std.mem.indexOf(u8, row_bytes[0..pos], needle) != null) {
                return true;
            }
        }
        return false;
    }
};

test "buffer get, set, setString ve UTF8 destegi" {
    const area = Rect.init(0, 0, 80, 24);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    const w = buf.setString(0, 0, "Omnitrix 🚀 Core", .{ .fg = .green }, 80);
    try std.testing.expect(w > 10);

    const c0 = buf.get(0, 0).?;
    try std.testing.expectEqualStrings("O", c0.getSymbol());
    try std.testing.expect(c0.style.fg.eql(.green));
}
