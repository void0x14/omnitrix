//! omnitrix-tui/editor/gap_buffer.zig
//!
//! Yüksek performanslı $O(1)$ Gap Buffer (Boşluk Tamponu) metin düzenleyici çekirdeği.
//! İmleç etrafındaki ekleme/silme operasyonlarını bellek kopyalaması yapmadan $O(1)$ hızında yürütür.
//! Unicode (UTF-8) kod noktası, bayt ofseti ve görsel sütun genişliği takibini tam doğrulukla yapar.

const std = @import("std");
const unicode_mod = @import("../unicode.zig");

pub const GapBuffer = struct {
    allocator: std.mem.Allocator,
    buffer: []u8,
    gap_start: usize,
    gap_end: usize,

    const INITIAL_CAPACITY: usize = 128;
    const MIN_GAP_GROWTH: usize = 64;

    pub fn init(allocator: std.mem.Allocator) !GapBuffer {
        const buf = try allocator.alloc(u8, INITIAL_CAPACITY);
        return .{
            .allocator = allocator,
            .buffer = buf,
            .gap_start = 0,
            .gap_end = INITIAL_CAPACITY,
        };
    }

    pub fn deinit(self: *GapBuffer) void {
        self.allocator.free(self.buffer);
        self.* = undefined;
    }

    /// Tampondaki toplam metin bayt uzunluğu (boşluk hariç).
    pub fn len(self: *const GapBuffer) usize {
        return self.buffer.len - (self.gap_end - self.gap_start);
    }

    /// Boşluk boyutu.
    pub fn gapLen(self: *const GapBuffer) usize {
        return self.gap_end - self.gap_start;
    }

    /// İmlecin mevcut bayt pozisyonu (0 .. len).
    pub fn cursor(self: *const GapBuffer) usize {
        return self.gap_start;
    }

    /// Boşluğu (ve dolayısıyla imleci) hedef bayt ofsetine taşır.
    pub fn moveCursorTo(self: *GapBuffer, target: usize) void {
        const total_len = self.len();
        const safe_target = @min(target, total_len);

        if (safe_target < self.gap_start) {
            // İmleci sola kaydır: gap_start öncesindeki baytları gap_end sonrasına taşı
            const count = self.gap_start - safe_target;
            const src = safe_target;
            const dst = self.gap_end - count;
            @memcpy(self.buffer[dst .. dst + count], self.buffer[src .. src + count]);
            self.gap_start = safe_target;
            self.gap_end = dst;
        } else if (safe_target > self.gap_start) {
            // İmleci sağa kaydır: gap_end sonrasındaki baytları gap_start pozisyonuna taşı
            const count = safe_target - self.gap_start;
            const src = self.gap_end;
            const dst = self.gap_start;
            @memcpy(self.buffer[dst .. dst + count], self.buffer[src .. src + count]);
            self.gap_start += count;
            self.gap_end += count;
        }
    }

    /// İmleç pozisyonuna tek bir UTF-8 bayt ekler.
    pub fn insertByte(self: *GapBuffer, byte: u8) !void {
        try self.ensureCapacity(1);
        self.buffer[self.gap_start] = byte;
        self.gap_start += 1;
    }

    /// İmleç pozisyonuna UTF-8 metin dilimi ekler.
    pub fn insertSlice(self: *GapBuffer, slice: []const u8) !void {
        if (slice.len == 0) return;
        try self.ensureCapacity(slice.len);
        @memcpy(self.buffer[self.gap_start .. self.gap_start + slice.len], slice);
        self.gap_start += slice.len;
    }

    /// İmleç pozisyonuna bir Unicode kod noktası (char) ekler.
    pub fn insertChar(self: *GapBuffer, cp: u21) !void {
        var utf8_buf: [4]u8 = undefined;
        const n = try std.unicode.utf8Encode(cp, &utf8_buf);
        try self.insertSlice(utf8_buf[0..n]);
    }

    /// İmlecin solundaki bir UTF-8 karakteri siler (Backspace).
    pub fn backspace(self: *GapBuffer) bool {
        if (self.gap_start == 0) return false;

        // Geriye doğru geçerli bir UTF-8 başlangıç baytı bul
        var count: usize = 1;
        while (count < 4 and self.gap_start >= count) {
            const b = self.buffer[self.gap_start - count];
            // 10xxxxxx (devam baytı) değilse başlangıç baytıdır
            if ((b & 0xC0) != 0x80) {
                break;
            }
            count += 1;
        }

        self.gap_start -= count;
        return true;
    }

    /// İmlecin sağındaki bir UTF-8 karakteri siler (Delete).
    pub fn delete(self: *GapBuffer) bool {
        if (self.gap_end >= self.buffer.len) return false;

        const first_byte = self.buffer[self.gap_end];
        const seq_len = std.unicode.utf8ByteSequenceLength(first_byte) catch 1;
        const safe_len = @min(seq_len, self.buffer.len - self.gap_end);

        self.gap_end += safe_len;
        return true;
    }

    /// Tampondaki tüm metni sıfırlar.
    pub fn clear(self: *GapBuffer) void {
        self.gap_start = 0;
        self.gap_end = self.buffer.len;
    }

    /// Tüm metni tek bir tahsis edilmiş UTF-8 dilimi olarak döner.
    pub fn toString(self: *const GapBuffer, allocator: std.mem.Allocator) ![]u8 {
        const total = self.len();
        const res = try allocator.alloc(u8, total);
        const prefix = self.buffer[0..self.gap_start];
        const suffix = self.buffer[self.gap_end..self.buffer.len];

        @memcpy(res[0..prefix.len], prefix);
        @memcpy(res[prefix.len..total], suffix);
        return res;
    }

    /// Metni satırlara bölerek liste olarak döner.
    pub fn getLines(self: *const GapBuffer, allocator: std.mem.Allocator) !std.ArrayList([]u8) {
        var lines = std.ArrayList([]u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        const full_text = try self.toString(allocator);
        defer allocator.free(full_text);

        var it = std.mem.splitScalar(u8, full_text, '\n');
        while (it.next()) |line| {
            try lines.append(allocator, try allocator.dupe(u8, line));
        }

        if (lines.items.len == 0) {
            try lines.append(allocator, try allocator.dupe(u8, ""));
        }

        return lines;
    }

    /// İmlecin satır ve sütun numarasını döner (0 tabanlı).
    pub fn getCursorLineCol(self: *const GapBuffer) struct { line: usize, col: usize } {
        var line: usize = 0;
        var last_line_start: usize = 0;

        var i: usize = 0;
        while (i < self.gap_start) {
            const b = self.buffer[i];
            if (b == '\n') {
                line += 1;
                last_line_start = i + 1;
            }
            i += 1;
        }

        // Sütun için UTF-8 görsel genişliğini hesapla
        const line_slice = self.buffer[last_line_start..self.gap_start];
        const col = unicode_mod.strWidth(line_slice);

        return .{ .line = line, .col = col };
    }

    fn ensureCapacity(self: *GapBuffer, needed: usize) !void {
        if (self.gapLen() >= needed) return;

        const current_len = self.len();
        const growth = @max(needed + MIN_GAP_GROWTH, self.buffer.len);
        const new_cap = self.buffer.len + growth;
        const new_buf = try self.allocator.alloc(u8, new_cap);

        // Prefix'i kopyala (0 .. gap_start)
        @memcpy(new_buf[0..self.gap_start], self.buffer[0..self.gap_start]);

        // Suffix'i sona kopyala
        const suffix_len = self.buffer.len - self.gap_end;
        const new_gap_end = new_cap - suffix_len;
        @memcpy(new_buf[new_gap_end..new_cap], self.buffer[self.gap_end..self.buffer.len]);

        self.allocator.free(self.buffer);
        self.buffer = new_buf;
        self.gap_end = new_gap_end;

        std.debug.assert(self.len() == current_len);
    }
};

// -----------------------------------------------------------------------------
// Unit Testler
// -----------------------------------------------------------------------------

test "gap buffer: insert, delete, move cursor, utf8" {
    var gb = try GapBuffer.init(std.testing.allocator);
    defer gb.deinit();

    try gb.insertSlice("Hello World");
    try std.testing.expectEqual(@as(usize, 11), gb.len());

    // İmleci "Hello" sonrasına taşı (offset 5)
    gb.moveCursorTo(5);
    try gb.insertSlice(" Brave");

    const text1 = try gb.toString(std.testing.allocator);
    defer std.testing.allocator.free(text1);
    try std.testing.expectEqualStrings("Hello Brave World", text1);

    // Backspace: " Brave" sil
    gb.moveCursorTo(11);
    var i: usize = 0;
    while (i < 6) : (i += 1) {
        _ = gb.backspace();
    }

    const text2 = try gb.toString(std.testing.allocator);
    defer std.testing.allocator.free(text2);
    try std.testing.expectEqualStrings("Hello World", text2);

    // Türkçe UTF-8 ekle
    try gb.insertSlice(" 🚀 Türkçe");
    const text3 = try gb.toString(std.testing.allocator);
    defer std.testing.allocator.free(text3);
    try std.testing.expect(std.mem.indexOf(u8, text3, "Türkçe") != null);
}
