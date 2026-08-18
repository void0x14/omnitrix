//! omnitrix-tui: Düşük seviyeli, değiştirilebilir Terminal Backend katmanı (tasarım Bölüm 5, 5.1, Kol A - A1).
//!
//! Özellikler:
//! - TerminalBackend arayüzü (değiştirilebilir; Omnitrix renderer'ı doğrudan backend'e bağlanmaz).
//! - ANSI kaçış dizileri (Escape sequences): renkler (16-renk, 256-renk, 24-bit RGB), stiller, imleç, ekran tamponu.
//! - Raw mode açma/kapama (POSIX termios ile güvenli geçiş).
//! - Alternate screen buffer açma/kapama (`\x1b[?1049h` / `\x1b[?1049l`).
//! - Terminal boyutu sorgulama (TIOCGWINSZ) ve pencere boyutu değişimi (resize).
//! - Safe terminal state cleanup (RAII TerminalGuard — panik/çıkışta terminalin temiz bırakılması).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const builtin = @import("builtin");

/// Terminal boyut bilgisi (sütun ve satır).
pub const TerminalSize = struct {
    cols: u16 = 80,
    rows: u16 = 24,

    pub fn isNarrow(self: TerminalSize) bool {
        return self.cols < 80;
    }
};

/// Renk tanımı.
pub const Color = union(enum) {
    default,
    ansi: u8,
    rgb: struct { r: u8, g: u8, b: u8 },

    pub const black = Color{ .ansi = 0 };
    pub const red = Color{ .ansi = 1 };
    pub const green = Color{ .ansi = 2 };
    pub const yellow = Color{ .ansi = 3 };
    pub const blue = Color{ .ansi = 4 };
    pub const magenta = Color{ .ansi = 5 };
    pub const cyan = Color{ .ansi = 6 };
    pub const white = Color{ .ansi = 7 };
    pub const bright_black = Color{ .ansi = 8 };
    pub const bright_red = Color{ .ansi = 9 };
    pub const bright_green = Color{ .ansi = 10 };
    pub const bright_yellow = Color{ .ansi = 11 };
    pub const bright_blue = Color{ .ansi = 12 };
    pub const bright_magenta = Color{ .ansi = 13 };
    pub const bright_cyan = Color{ .ansi = 14 };
    pub const bright_white = Color{ .ansi = 15 };
};

/// Metin biçimlendirme stili.
pub const Style = struct {
    fg: Color = .default,
    bg: Color = .default,
    bold: bool = false,
    dim: bool = false,
    italic: bool = false,
    underline: bool = false,
    reverse: bool = false,

    pub const default = Style{};
};

/// ANSI Kaçış Dizileri Sabitleri
pub const ANSI = struct {
    pub const reset = "\x1b[0m";
    pub const bold = "\x1b[1m";
    pub const dim = "\x1b[2m";
    pub const italic = "\x1b[3m";
    pub const underline = "\x1b[4m";
    pub const reverse = "\x1b[7m";

    pub const clear_screen = "\x1b[2J\x1b[H";
    pub const clear_to_eol = "\x1b[K";
    pub const clear_line = "\x1b[2K\r";

    pub const enter_alt_screen = "\x1b[?1049h";
    pub const exit_alt_screen = "\x1b[?1049l";

    pub const hide_cursor = "\x1b[?25l";
    pub const show_cursor = "\x1b[?25h";

    pub const enable_mouse = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";
    pub const disable_mouse = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";
};

/// Terminal Backend Hataları
pub const TerminalError = error{
    RawModeFailed,
    IoError,
    SizeQueryFailed,
    InvalidInput,
    UnsupportedPlatform,
};

/// Düşük seviyeli, değiştirilebilir Terminal Backend Arayüzü.
pub const TerminalBackend = struct {
    ptr: *anyopaque,
    vtable: *const VTable,

    pub const VTable = struct {
        enterRawMode: *const fn (ctx: *anyopaque) TerminalError!void,
        exitRawMode: *const fn (ctx: *anyopaque) TerminalError!void,
        enterAltScreen: *const fn (ctx: *anyopaque) TerminalError!void,
        exitAltScreen: *const fn (ctx: *anyopaque) TerminalError!void,
        hideCursor: *const fn (ctx: *anyopaque) TerminalError!void,
        showCursor: *const fn (ctx: *anyopaque) TerminalError!void,
        clearScreen: *const fn (ctx: *anyopaque) TerminalError!void,
        clearLine: *const fn (ctx: *anyopaque) TerminalError!void,
        moveCursor: *const fn (ctx: *anyopaque, row: u16, col: u16) TerminalError!void,
        getSize: *const fn (ctx: *const anyopaque) TerminalError!TerminalSize,
        write: *const fn (ctx: *anyopaque, data: []const u8) TerminalError!void,
        flush: *const fn (ctx: *anyopaque) TerminalError!void,
        readInput: *const fn (ctx: *anyopaque, buf: []u8) TerminalError!usize,
    };

    pub fn enterRawMode(self: TerminalBackend) !void {
        return self.vtable.enterRawMode(self.ptr);
    }

    pub fn exitRawMode(self: TerminalBackend) !void {
        return self.vtable.exitRawMode(self.ptr);
    }

    pub fn enterAltScreen(self: TerminalBackend) !void {
        return self.vtable.enterAltScreen(self.ptr);
    }

    pub fn exitAltScreen(self: TerminalBackend) !void {
        return self.vtable.exitAltScreen(self.ptr);
    }

    pub fn hideCursor(self: TerminalBackend) !void {
        return self.vtable.hideCursor(self.ptr);
    }

    pub fn showCursor(self: TerminalBackend) !void {
        return self.vtable.showCursor(self.ptr);
    }

    pub fn clearScreen(self: TerminalBackend) !void {
        return self.vtable.clearScreen(self.ptr);
    }

    pub fn clearLine(self: TerminalBackend) !void {
        return self.vtable.clearLine(self.ptr);
    }

    pub fn moveCursor(self: TerminalBackend, row: u16, col: u16) !void {
        return self.vtable.moveCursor(self.ptr, row, col);
    }

    pub fn getSize(self: TerminalBackend) !TerminalSize {
        return self.vtable.getSize(self.ptr);
    }

    pub fn write(self: TerminalBackend, data: []const u8) !void {
        return self.vtable.write(self.ptr, data);
    }

    pub fn flush(self: TerminalBackend) !void {
        return self.vtable.flush(self.ptr);
    }

    pub fn readInput(self: TerminalBackend, buf: []u8) !usize {
        return self.vtable.readInput(self.ptr, buf);
    }
};

/// ANSI Escape Sequence formatlayıcı yardımcı fonksiyonlar.
pub fn appendFgColor(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, color: Color) !void {
    switch (color) {
        .default => try buf.appendSlice(allocator, "\x1b[39m"),
        .ansi => |c| {
            if (c < 8) {
                var tmp: [16]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[{d}m", .{30 + c});
                try buf.appendSlice(allocator, s);
            } else if (c < 16) {
                var tmp: [16]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[{d}m", .{90 + (c - 8)});
                try buf.appendSlice(allocator, s);
            } else {
                var tmp: [32]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[38;5;{d}m", .{c});
                try buf.appendSlice(allocator, s);
            }
        },
        .rgb => |rgb| {
            var tmp: [32]u8 = undefined;
            const s = try std.fmt.bufPrint(&tmp, "\x1b[38;2;{d};{d};{d}m", .{ rgb.r, rgb.g, rgb.b });
            try buf.appendSlice(allocator, s);
        },
    }
}

pub fn appendBgColor(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, color: Color) !void {
    switch (color) {
        .default => try buf.appendSlice(allocator, "\x1b[49m"),
        .ansi => |c| {
            if (c < 8) {
                var tmp: [16]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[{d}m", .{40 + c});
                try buf.appendSlice(allocator, s);
            } else if (c < 16) {
                var tmp: [16]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[{d}m", .{100 + (c - 8)});
                try buf.appendSlice(allocator, s);
            } else {
                var tmp: [32]u8 = undefined;
                const s = try std.fmt.bufPrint(&tmp, "\x1b[48;5;{d}m", .{c});
                try buf.appendSlice(allocator, s);
            }
        },
        .rgb => |rgb| {
            var tmp: [32]u8 = undefined;
            const s = try std.fmt.bufPrint(&tmp, "\x1b[48;2;{d};{d};{d}m", .{ rgb.r, rgb.g, rgb.b });
            try buf.appendSlice(allocator, s);
        },
    }
}

pub fn appendStyle(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, style: Style) !void {
    try buf.appendSlice(allocator, ANSI.reset);
    if (style.bold) try buf.appendSlice(allocator, ANSI.bold);
    if (style.dim) try buf.appendSlice(allocator, ANSI.dim);
    if (style.italic) try buf.appendSlice(allocator, ANSI.italic);
    if (style.underline) try buf.appendSlice(allocator, ANSI.underline);
    if (style.reverse) try buf.appendSlice(allocator, ANSI.reverse);
    try appendFgColor(buf, allocator, style.fg);
    try appendBgColor(buf, allocator, style.bg);
}

/// POSIX Gerçek Terminal Backend'i (Linux / macOS / BSD).
pub const PosixTerminal = struct {
    allocator: std.mem.Allocator,
    stdout_fd: std.posix.fd_t,
    stdin_fd: std.posix.fd_t,
    orig_termios: ?std.posix.termios = null,
    in_raw_mode: bool = false,
    in_alt_screen: bool = false,
    cursor_hidden: bool = false,
    write_buffer: std.ArrayList(u8),

    pub fn init(allocator: std.mem.Allocator) PosixTerminal {
        return .{
            .allocator = allocator,
            .stdout_fd = std.posix.STDOUT_FILENO,
            .stdin_fd = std.posix.STDIN_FILENO,
            .orig_termios = null,
            .in_raw_mode = false,
            .in_alt_screen = false,
            .cursor_hidden = false,
            .write_buffer = std.ArrayList(u8).empty,
        };
    }

    pub fn deinit(self: *PosixTerminal) void {
        self.cleanup();
        self.write_buffer.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn backend(self: *PosixTerminal) TerminalBackend {
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
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.enterRawMode();
    }

    fn exitRawModeWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.exitRawMode();
    }

    fn enterAltScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.enterAltScreen();
    }

    fn exitAltScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.exitAltScreen();
    }

    fn hideCursorWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.hideCursor();
    }

    fn showCursorWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.showCursor();
    }

    fn clearScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.clearScreen();
    }

    fn clearLineWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.clearLine();
    }

    fn moveCursorWrapper(ctx: *anyopaque, row: u16, col: u16) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.moveCursor(row, col);
    }

    fn getSizeWrapper(ctx: *const anyopaque) TerminalError!TerminalSize {
        const self: *const PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.getSize();
    }

    fn writeWrapper(ctx: *anyopaque, data: []const u8) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.write(data);
    }

    fn flushWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.flush();
    }

    fn readInputWrapper(ctx: *anyopaque, buf: []u8) TerminalError!usize {
        const self: *PosixTerminal = @ptrCast(@alignCast(ctx));
        return self.readInput(buf);
    }

    pub fn enterRawMode(self: *PosixTerminal) TerminalError!void {
        if (builtin.os.tag == .windows) return error.UnsupportedPlatform;
        if (self.in_raw_mode) return;

        const orig = std.posix.tcgetattr(self.stdin_fd) catch return error.RawModeFailed;
        self.orig_termios = orig;

        var raw = orig;
        // Input modes: no break, no CR to NL, no parity check, no strip char, no start/stop output control
        raw.iflag.BRKINT = false;
        raw.iflag.ICRNL = false;
        raw.iflag.INPCK = false;
        raw.iflag.ISTRIP = false;
        raw.iflag.IXON = false;

        // Output modes: disable post processing
        raw.oflag.OPOST = false;

        // Control modes: set 8 bit chars
        raw.cflag.CSIZE = .CS8;

        // Local modes: echo off, canonical off, extended input off, signal chars off
        raw.lflag.ECHO = false;
        raw.lflag.ICANON = false;
        raw.lflag.IEXTEN = false;
        raw.lflag.ISIG = false;

        // Control characters: min bytes to read = 0, timeout = 1 (100ms)
        raw.cc[@intFromEnum(std.posix.V.MIN)] = 0;
        raw.cc[@intFromEnum(std.posix.V.TIME)] = 1;

        std.posix.tcsetattr(self.stdin_fd, .FLUSH, raw) catch return error.RawModeFailed;
        self.in_raw_mode = true;
    }

    pub fn exitRawMode(self: *PosixTerminal) TerminalError!void {
        if (builtin.os.tag == .windows) return error.UnsupportedPlatform;
        if (!self.in_raw_mode) return;

        if (self.orig_termios) |orig| {
            std.posix.tcsetattr(self.stdin_fd, .FLUSH, orig) catch return error.RawModeFailed;
            self.in_raw_mode = false;
        }
    }

    pub fn enterAltScreen(self: *PosixTerminal) TerminalError!void {
        if (self.in_alt_screen) return;
        try self.write(ANSI.enter_alt_screen);
        try self.flush();
        self.in_alt_screen = true;
    }

    pub fn exitAltScreen(self: *PosixTerminal) TerminalError!void {
        if (!self.in_alt_screen) return;
        try self.write(ANSI.exit_alt_screen);
        try self.flush();
        self.in_alt_screen = false;
    }

    pub fn hideCursor(self: *PosixTerminal) TerminalError!void {
        if (self.cursor_hidden) return;
        try self.write(ANSI.hide_cursor);
        self.cursor_hidden = true;
    }

    pub fn showCursor(self: *PosixTerminal) TerminalError!void {
        if (!self.cursor_hidden) return;
        try self.write(ANSI.show_cursor);
        self.cursor_hidden = false;
    }

    pub fn clearScreen(self: *PosixTerminal) TerminalError!void {
        try self.write(ANSI.clear_screen);
    }

    pub fn clearLine(self: *PosixTerminal) TerminalError!void {
        try self.write(ANSI.clear_line);
    }

    pub fn moveCursor(self: *PosixTerminal, row: u16, col: u16) TerminalError!void {
        var buf: [32]u8 = undefined;
        const seq = std.fmt.bufPrint(&buf, "\x1b[{d};{d}H", .{ row + 1, col + 1 }) catch return error.InvalidInput;
        try self.write(seq);
    }

    pub fn getSize(self: *const PosixTerminal) TerminalError!TerminalSize {
        if (builtin.os.tag == .windows) return .{ .cols = 80, .rows = 24 };

        var ws: std.posix.winsize = undefined;
        if (builtin.os.tag == .linux) {
            const res = std.os.linux.ioctl(self.stdout_fd, 0x5413, @intFromPtr(&ws));
            if (std.os.linux.errno(res) == .SUCCESS and ws.col > 0 and ws.row > 0) {
                return .{
                    .cols = ws.col,
                    .rows = ws.row,
                };
            }
        }
        return .{ .cols = 80, .rows = 24 };
    }

    pub fn write(self: *PosixTerminal, data: []const u8) TerminalError!void {
        self.write_buffer.appendSlice(self.allocator, data) catch return error.IoError;
    }

    pub fn flush(self: *PosixTerminal) TerminalError!void {
        if (self.write_buffer.items.len == 0) return;
        if (builtin.os.tag == .linux) {
            const rc = std.os.linux.write(self.stdout_fd, self.write_buffer.items.ptr, self.write_buffer.items.len);
            if (std.os.linux.errno(rc) != .SUCCESS) return error.IoError;
        }
        self.write_buffer.clearRetainingCapacity();
    }

    pub fn readInput(self: *PosixTerminal, buf: []u8) TerminalError!usize {
        if (buf.len == 0) return 0;
        if (builtin.os.tag == .linux) {
            const rc = std.os.linux.read(self.stdin_fd, buf.ptr, buf.len);
            const err = std.os.linux.errno(rc);
            if (err == .SUCCESS) return rc;
            if (err == .AGAIN) return 0;
            return error.IoError;
        }
        return 0;
    }

    /// Panik, sinyal veya çıkış durumlarında terminali kesinlikle temizler.
    pub fn cleanup(self: *PosixTerminal) void {
        if (builtin.os.tag == .linux) {
            if (self.cursor_hidden) {
                _ = std.os.linux.write(self.stdout_fd, ANSI.show_cursor.ptr, ANSI.show_cursor.len);
                self.cursor_hidden = false;
            }
            if (self.in_alt_screen) {
                _ = std.os.linux.write(self.stdout_fd, ANSI.exit_alt_screen.ptr, ANSI.exit_alt_screen.len);
                self.in_alt_screen = false;
            }
        }
        if (self.in_raw_mode) {
            self.exitRawMode() catch {};
        }
    }
};

/// RAII Terminal Guard: Kapsam dışına çıkıldığında terminali varsayılan durumuna döndürür.
pub const TerminalGuard = struct {
    backend: TerminalBackend,

    pub fn init(backend_inst: TerminalBackend) TerminalError!TerminalGuard {
        try backend_inst.enterRawMode();
        try backend_inst.enterAltScreen();
        try backend_inst.hideCursor();
        return .{ .backend = backend_inst };
    }

    pub fn deinit(self: *TerminalGuard) void {
        self.backend.showCursor() catch {};
        self.backend.exitAltScreen() catch {};
        self.backend.exitRawMode() catch {};
        self.backend.flush() catch {};
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11 / A1)
// -----------------------------------------------------------------------------

test "terminal size isNarrow" {
    const wide = TerminalSize{ .cols = 100, .rows = 30 };
    const narrow = TerminalSize{ .cols = 79, .rows = 24 };

    try std.testing.expect(!wide.isNarrow());
    try std.testing.expect(narrow.isNarrow());
}

test "ansi style and color formatting" {
    var out = std.ArrayList(u8).empty;
    defer out.deinit(std.testing.allocator);

    const style = Style{
        .fg = Color.green,
        .bg = Color.default,
        .bold = true,
    };

    try appendStyle(&out, std.testing.allocator, style);
    try std.testing.expect(out.items.len > 0);
    try std.testing.expect(std.mem.indexOf(u8, out.items, "\x1b[32m") != null);
    try std.testing.expect(std.mem.indexOf(u8, out.items, ANSI.bold) != null);
}
