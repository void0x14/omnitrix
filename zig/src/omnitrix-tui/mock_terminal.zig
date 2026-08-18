//! omnitrix-tui: Deterministik Unit Testler için Mock/Virtual Terminal Backend'i (tasarım Bölüm 5.1, Kol A - A1, Doğrulama 11).
//!
//! Özellikler:
//! - TerminalBackend arayüzünü birebir uygular.
//! - Ekran matrisi (2D Cell grid) üzerinde karakter ve renk durumu tutar.
//! - ANSI kaçış dizilerini ve düz metinleri sanal ekrana parse edip çizer.
//! - Sentetik resize olayları (`resize(cols, rows)`).
//! - Sentetik tuş ve girdi kuyruğu (`feedInput(bytes)`).
//! - Ekran metnini denetleme yardımcıları (`getLine`, `getScreenText`, `containsText`).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const term = @import("terminal.zig");
const unicode = @import("unicode.zig");

pub const TerminalSize = term.TerminalSize;
pub const TerminalBackend = term.TerminalBackend;
pub const TerminalError = term.TerminalError;
pub const Color = term.Color;
pub const Style = term.Style;
pub const ANSI = term.ANSI;

/// Sanal ekran hücresi
pub const Cell = struct {
    char: u21 = ' ',
    style: Style = .default,
};

pub const MockTerminal = struct {
    allocator: std.mem.Allocator,
    size: TerminalSize,
    raw_mode: bool = false,
    alt_screen: bool = false,
    cursor_visible: bool = true,
    cursor_row: u16 = 0,
    cursor_col: u16 = 0,
    current_style: Style = .default,

    grid: [][]Cell,
    written_raw: std.ArrayList(u8),
    input_queue: std.ArrayList(u8),

    pub fn init(allocator: std.mem.Allocator, cols: u16, rows: u16) !MockTerminal {
        var grid = try allocator.alloc([]Cell, rows);
        errdefer allocator.free(grid);

        for (grid, 0..) |*row, r_idx| {
            row.* = try allocator.alloc(Cell, cols);
            for (row.*) |*c| {
                c.* = .{ .char = ' ', .style = .default };
            }
            errdefer {
                for (grid[0..r_idx]) |prev_row| allocator.free(prev_row);
            }
        }

        return .{
            .allocator = allocator,
            .size = .{ .cols = cols, .rows = rows },
            .raw_mode = false,
            .alt_screen = false,
            .cursor_visible = true,
            .cursor_row = 0,
            .cursor_col = 0,
            .current_style = .default,
            .grid = grid,
            .written_raw = std.ArrayList(u8).empty,
            .input_queue = std.ArrayList(u8).empty,
        };
    }

    pub fn deinit(self: *MockTerminal) void {
        for (self.grid) |row| {
            self.allocator.free(row);
        }
        self.allocator.free(self.grid);
        self.written_raw.deinit(self.allocator);
        self.input_queue.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn backend(self: *MockTerminal) TerminalBackend {
        return .{
            .ptr = self,
            .vtable = &vtable,
        };
    }

    const vtable = TerminalBackend.VTable{
        .enterRawMode = enterRawModeWrapper,
        .exitRawMode = exitRawModeWrapper,
        .enterAltScreen = enterAltScreenWrapper,
        .exitAltScreen = exitAltScreenWrapper,
        .hideCursor = hideCursorWrapper,
        .showCursor = showCursorWrapper,
        .clearScreen = clearScreenWrapper,
        .clearLine = clearLineWrapper,
        .moveCursor = moveCursorWrapper,
        .getSize = getSizeWrapper,
        .write = writeWrapper,
        .flush = flushWrapper,
        .readInput = readInputWrapper,
    };

    fn enterRawModeWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.raw_mode = true;
    }

    fn exitRawModeWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.raw_mode = false;
    }

    fn enterAltScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.alt_screen = true;
        self.clearGrid();
    }

    fn exitAltScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.alt_screen = false;
        self.clearGrid();
    }

    fn hideCursorWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.cursor_visible = false;
    }

    fn showCursorWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.cursor_visible = true;
    }

    fn clearScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.clearGrid();
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn clearLineWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        if (self.cursor_row < self.size.rows) {
            const row = self.grid[self.cursor_row];
            for (row) |*c| c.* = .{ .char = ' ', .style = self.current_style };
        }
        self.cursor_col = 0;
    }

    fn moveCursorWrapper(ctx: *anyopaque, row: u16, col: u16) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.cursor_row = @min(row, if (self.size.rows > 0) self.size.rows - 1 else 0);
        self.cursor_col = @min(col, if (self.size.cols > 0) self.size.cols - 1 else 0);
    }

    fn getSizeWrapper(ctx: *const anyopaque) TerminalError!TerminalSize {
        const self: *const MockTerminal = @ptrCast(@alignCast(ctx));
        return self.size;
    }

    fn writeWrapper(ctx: *anyopaque, data: []const u8) TerminalError!void {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        self.written_raw.appendSlice(self.allocator, data) catch return error.IoError;
        self.parseAndDraw(data);
    }

    fn flushWrapper(ctx: *anyopaque) TerminalError!void {
        _ = ctx;
    }

    fn readInputWrapper(ctx: *anyopaque, buf: []u8) TerminalError!usize {
        const self: *MockTerminal = @ptrCast(@alignCast(ctx));
        if (self.input_queue.items.len == 0) return 0;

        const count = @min(buf.len, self.input_queue.items.len);
        @memcpy(buf[0..count], self.input_queue.items[0..count]);

        // Okunanları kuyruktan çıkar
        const rem = self.input_queue.items.len - count;
        if (rem > 0) {
            std.mem.copyForwards(u8, self.input_queue.items[0..rem], self.input_queue.items[count..]);
        }
        self.input_queue.items.len = rem;

        return count;
    }

    /// Test için sentetik girdi ekler (ör. tuş basımı veya kaçış dizisi)
    pub fn feedInput(self: *MockTerminal, bytes: []const u8) !void {
        try self.input_queue.appendSlice(self.allocator, bytes);
    }

    /// Sentetik terminal yeniden boyutlandırması (resize simülasyonu)
    pub fn resize(self: *MockTerminal, new_cols: u16, new_rows: u16) !void {
        if (new_cols == self.size.cols and new_rows == self.size.rows) return;

        var new_grid = try self.allocator.alloc([]Cell, new_rows);
        errdefer self.allocator.free(new_grid);

        for (new_grid, 0..) |*row, r_idx| {
            row.* = try self.allocator.alloc(Cell, new_cols);
            for (row.*) |*c| c.* = .{ .char = ' ', .style = .default };
            errdefer {
                for (new_grid[0..r_idx]) |prev_row| self.allocator.free(prev_row);
            }
        }

        // Eski grid içeriğini yeni alana kopyala
        const copy_rows = @min(self.size.rows, new_rows);
        const copy_cols = @min(self.size.cols, new_cols);
        for (0..copy_rows) |r| {
            @memcpy(new_grid[r][0..copy_cols], self.grid[r][0..copy_cols]);
        }

        for (self.grid) |row| self.allocator.free(row);
        self.allocator.free(self.grid);

        self.grid = new_grid;
        self.size = .{ .cols = new_cols, .rows = new_rows };
        self.cursor_row = @min(self.cursor_row, if (new_rows > 0) new_rows - 1 else 0);
        self.cursor_col = @min(self.cursor_col, if (new_cols > 0) new_cols - 1 else 0);
    }

    pub fn clearGrid(self: *MockTerminal) void {
        for (self.grid) |row| {
            for (row) |*c| c.* = .{ .char = ' ', .style = .default };
        }
    }

    /// Yazılan metinleri ve basit ANSI kaçış dizilerini sanal ızgaraya işler
    fn parseAndDraw(self: *MockTerminal, data: []const u8) void {
        var i: usize = 0;
        while (i < data.len) {
            if (data[i] == '\x1b') {
                // Kaçış dizisini atla / yorumla
                if (i + 1 < data.len and data[i + 1] == '[') {
                    var end_idx = i + 2;
                    while (end_idx < data.len and (data[end_idx] >= '0' and data[end_idx] <= '?')) : (end_idx += 1) {}
                    if (end_idx < data.len) {
                        const cmd = data[end_idx];
                        const seq = data[i .. end_idx + 1];

                        if (cmd == 'H') {
                            // \x1b[row;colH imleç konumu
                            var it = std.mem.splitScalar(u8, seq[2 .. seq.len - 1], ';');
                            if (it.next()) |r_str| {
                                const r = std.fmt.parseInt(u16, r_str, 10) catch 1;
                                self.cursor_row = if (r > 0) r - 1 else 0;
                            }
                            if (it.next()) |c_str| {
                                const c = std.fmt.parseInt(u16, c_str, 10) catch 1;
                                self.cursor_col = if (c > 0) c - 1 else 0;
                            }
                        } else if (cmd == 'J') {
                            self.clearGrid();
                        } else if (cmd == 'K') {
                            if (self.cursor_row < self.size.rows) {
                                var col = self.cursor_col;
                                while (col < self.size.cols) : (col += 1) {
                                    self.grid[self.cursor_row][col] = .{ .char = ' ', .style = self.current_style };
                                }
                            }
                        }
                        i = end_idx + 1;
                        continue;
                    }
                }
                i += 1;
                continue;
            }

            if (data[i] == '\n') {
                if (self.cursor_row + 1 >= self.size.rows) {
                    if (self.size.rows > 1) {
                        for (0..self.size.rows - 1) |r| {
                            @memcpy(self.grid[r], self.grid[r + 1]);
                        }
                        for (self.grid[self.size.rows - 1]) |*c| {
                            c.* = .{ .char = ' ', .style = .default };
                        }
                    }
                    self.cursor_row = if (self.size.rows > 0) self.size.rows - 1 else 0;
                } else {
                    self.cursor_row += 1;
                }
                self.cursor_col = 0;
                i += 1;
                continue;
            }

            if (data[i] == '\r') {
                self.cursor_col = 0;
                i += 1;
                continue;
            }

            if (data[i] == '\t') {
                self.cursor_col = (self.cursor_col + 4) & ~@as(u16, 3);
                if (self.cursor_col >= self.size.cols) {
                    self.cursor_col = if (self.size.cols > 0) self.size.cols - 1 else 0;
                }
                i += 1;
                continue;
            }

            // UTF-8 karakter çizimi
            const seq_len = std.unicode.utf8ByteSequenceLength(data[i]) catch 1;
            if (i + seq_len > data.len) break;
            const slice = data[i .. i + seq_len];
            const cp = std.unicode.utf8Decode(slice) catch {
                i += 1;
                continue;
            };

            const w = unicode.codepointWidth(cp);
            if (self.cursor_row < self.size.rows and self.cursor_col < self.size.cols) {
                self.grid[self.cursor_row][self.cursor_col] = .{
                    .char = cp,
                    .style = self.current_style,
                };
            }

            self.cursor_col += w;
            if (self.cursor_col >= self.size.cols) {
                if (self.cursor_row + 1 < self.size.rows) {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                } else {
                    self.cursor_col = if (self.size.cols > 0) self.size.cols - 1 else 0;
                }
            }

            i += seq_len;
        }
    }

    /// Verilen satırdaki metni UTF-8 string olarak döner
    pub fn getLineAlloc(self: *const MockTerminal, allocator: std.mem.Allocator, row: u16) ![]u8 {
        if (row >= self.size.rows) return try allocator.dupe(u8, "");
        var buf = std.ArrayList(u8).empty;
        errdefer buf.deinit(allocator);

        for (self.grid[row]) |cell| {
            var char_buf: [4]u8 = undefined;
            const len = std.unicode.utf8Encode(cell.char, &char_buf) catch 0;
            try buf.appendSlice(allocator, char_buf[0..len]);
        }

        // Sondaki gereksiz boşlukları kırp
        var end = buf.items.len;
        while (end > 0 and buf.items[end - 1] == ' ') : (end -= 1) {}
        const trimmed = buf.items[0..end];
        const res = try allocator.dupe(u8, trimmed);
        buf.deinit(allocator);
        return res;
    }

    /// Tüm ekran görüntüsünü satır satır metin olarak birleştirir
    pub fn getScreenText(self: *const MockTerminal, allocator: std.mem.Allocator) ![]u8 {
        var buf = std.ArrayList(u8).empty;
        errdefer buf.deinit(allocator);

        for (0..self.size.rows) |r| {
            const line = try self.getLineAlloc(allocator, @intCast(r));
            defer allocator.free(line);
            try buf.appendSlice(allocator, line);
            try buf.append(allocator, '\n');
        }

        return try buf.toOwnedSlice(allocator);
    }

    /// Ekranda belirli bir metnin geçip geçmediğini kontrol eder
    pub fn containsText(self: *const MockTerminal, needle: []const u8) bool {
        for (0..self.size.rows) |r| {
            var row_buf: [512]u8 = undefined;
            var idx: usize = 0;
            for (self.grid[r]) |cell| {
                var char_buf: [4]u8 = undefined;
                const len = std.unicode.utf8Encode(cell.char, &char_buf) catch 0;
                if (idx + len < row_buf.len) {
                    @memcpy(row_buf[idx .. idx + len], char_buf[0..len]);
                    idx += len;
                }
            }
            if (std.mem.indexOf(u8, row_buf[0..idx], needle) != null) {
                return true;
            }
        }
        return false;
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11 / A1)
// -----------------------------------------------------------------------------

test "mock terminal yazi ve imlec yonetimi" {
    var mock = try MockTerminal.init(std.testing.allocator, 40, 10);
    defer mock.deinit();

    const be = mock.backend();
    try be.moveCursor(0, 0);
    try be.write("Omnitrix Runtime ⚡");

    try std.testing.expect(mock.containsText("Omnitrix Runtime ⚡"));
    const line0 = try mock.getLineAlloc(std.testing.allocator, 0);
    defer std.testing.allocator.free(line0);
    try std.testing.expectEqualStrings("Omnitrix Runtime ⚡", line0);
}

test "mock terminal resize ve girdi kuyrugu" {
    var mock = try MockTerminal.init(std.testing.allocator, 40, 10);
    defer mock.deinit();

    try mock.feedInput("hello\n");
    var buf: [16]u8 = undefined;
    const n = try mock.backend().readInput(&buf);
    try std.testing.expectEqual(@as(usize, 6), n);
    try std.testing.expectEqualStrings("hello\n", buf[0..n]);

    // Resize
    try mock.resize(80, 24);
    const size = try mock.backend().getSize();
    try std.testing.expectEqual(@as(u16, 80), size.cols);
    try std.testing.expectEqual(@as(u16, 24), size.rows);
}
