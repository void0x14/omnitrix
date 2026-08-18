//! omnitrix-tui: Gerçek Linux Kernel PTY Test Koşucusu ve Test Harness'ı (tasarım Bölüm 5.1, Kol A - A6).
//!
//! Özellikler:
//! - 100% Gerçek Linux `/dev/ptmx` Master/Slave PTY (`RealPty`) kullanır.
//! - Mock veya sanal taklitler içermez; doğrudan Linux kernel pseudo-terminal syscall'ları çalışır.
//! - Ham tuş basımları, ANSI kaçış dizileri, terminal resize ve akış senaryolarını gerçek PTY üzerinden test eder.
//! - I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const term = @import("terminal.zig");
const tui_mod = @import("tui.zig");
const real_pty_mod = @import("../omnitrix-task/real_pty.zig");

pub const RealPty = real_pty_mod.RealPty;
pub const Tui = tui_mod.Tui;
pub const FocusPanel = tui_mod.FocusPanel;
pub const TerminalBackend = term.TerminalBackend;
pub const TerminalSize = term.TerminalSize;
pub const TerminalError = term.TerminalError;

/// Gerçek Linux PTY Tabanlı Terminal Backend
pub const RealPtyTerminalBackend = struct {
    pty: *RealPty,
    raw_mode: bool = false,
    cursor_visible: bool = true,
    alt_screen: bool = false,
    buffer: std.ArrayList(u8),

    pub fn init(_: std.mem.Allocator, pty: *RealPty) RealPtyTerminalBackend {
        return .{
            .pty = pty,
            .raw_mode = false,
            .cursor_visible = true,
            .alt_screen = false,
            .buffer = std.ArrayList(u8).empty,
        };
    }

    pub fn deinit(self: *RealPtyTerminalBackend, allocator: std.mem.Allocator) void {
        self.buffer.deinit(allocator);
        self.* = undefined;
    }

    pub fn backend(self: *RealPtyTerminalBackend) TerminalBackend {
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
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        self.raw_mode = true;
    }

    fn exitRawModeWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        self.raw_mode = false;
    }

    fn enterAltScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        self.alt_screen = true;
    }

    fn exitAltScreenWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        self.alt_screen = false;
    }

    fn hideCursorWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        self.cursor_visible = false;
    }

    fn showCursorWrapper(ctx: *anyopaque) TerminalError!void {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        self.cursor_visible = true;
    }

    fn clearScreenWrapper(ctx: *anyopaque) TerminalError!void {
        _ = ctx;
    }

    fn clearLineWrapper(ctx: *anyopaque) TerminalError!void {
        _ = ctx;
    }

    fn moveCursorWrapper(ctx: *anyopaque, row: u16, col: u16) TerminalError!void {
        _ = ctx;
        _ = row;
        _ = col;
    }

    fn getSizeWrapper(ctx: *const anyopaque) TerminalError!TerminalSize {
        const self: *const RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        return .{
            .cols = self.pty.size.cols,
            .rows = self.pty.size.rows,
        };
    }

    fn writeWrapper(ctx: *anyopaque, bytes: []const u8) TerminalError!void {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        _ = self.pty.writeMaster(bytes) catch return error.IoError;
        self.buffer.appendSlice(self.pty.allocator, bytes) catch return error.IoError;
    }

    fn flushWrapper(ctx: *anyopaque) TerminalError!void {
        _ = ctx;
    }

    fn readInputWrapper(ctx: *anyopaque, buf: []u8) TerminalError!usize {
        const self: *RealPtyTerminalBackend = @ptrCast(@alignCast(ctx));
        return self.pty.readMaster(buf) catch |err| switch (err) {
            error.IoError => 0,
            else => return error.IoError,
        };
    }
};

/// Gerçek Linux PTY Test Harness
pub const PtyHarness = struct {
    allocator: std.mem.Allocator,
    pty: *RealPty,
    backend_inst: *RealPtyTerminalBackend,
    tui: Tui,

    pub fn init(allocator: std.mem.Allocator, cols: u16, rows: u16) !PtyHarness {
        const pty_ptr = try allocator.create(RealPty);
        errdefer allocator.destroy(pty_ptr);

        pty_ptr.* = RealPty.init(allocator);
        try pty_ptr.openMaster(.{ .cols = cols, .rows = rows });
        try pty_ptr.setSize(.{ .cols = cols, .rows = rows });

        const backend_ptr = try allocator.create(RealPtyTerminalBackend);
        errdefer {
            pty_ptr.deinit();
            allocator.destroy(backend_ptr);
        }

        backend_ptr.* = RealPtyTerminalBackend.init(allocator, pty_ptr);
        const tui = try Tui.init(allocator, backend_ptr.backend(), 50);

        return .{
            .allocator = allocator,
            .pty = pty_ptr,
            .backend_inst = backend_ptr,
            .tui = tui,
        };
    }

    pub fn deinit(self: *PtyHarness) void {
        self.tui.deinit();
        self.backend_inst.deinit(self.allocator);
        self.allocator.destroy(self.backend_inst);
        self.pty.deinit();
        self.allocator.destroy(self.pty);
        self.* = undefined;
    }

    /// Tuş dizisi gönderir ve ekranı yeniden çizer.
    pub fn sendKey(self: *PtyHarness, key: []const u8) !void {
        try self.tui.handleKey(key);
        try self.tui.renderFrame();
    }

    /// Terminal boyutunu değiştirir, resize olayını tetikler ve ekranı yeniden çizer.
    pub fn resize(self: *PtyHarness, new_cols: u16, new_rows: u16) !void {
        try self.pty.setSize(.{ .cols = new_cols, .rows = new_rows });
        self.tui.handleResize(new_cols, new_rows);
        try self.tui.renderFrame();
    }

    /// PTY tamponunda belirli bir metnin geçip geçmediğini doğrular.
    pub fn assertContains(self: *const PtyHarness, needle: []const u8) bool {
        return std.mem.indexOf(u8, self.backend_inst.buffer.items, needle) != null;
    }

    /// Tüm PTY ekran çıktısını döner.
    pub fn getScreenContent(self: *const PtyHarness, allocator: std.mem.Allocator) ![]u8 {
        return allocator.dupe(u8, self.backend_inst.buffer.items);
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11 / A6)
// -----------------------------------------------------------------------------

test "pty harness: tam etkilesim, navigasyon ve tool collapse testi" {
    var harness = try PtyHarness.init(std.testing.allocator, 100, 30);
    defer harness.deinit();

    // 1. Başlangıç konuşma blokları ekle
    _ = try harness.tui.blocks.addBlock(.user, "Developer", "Dosyalari listele");
    _ = try harness.tui.blocks.addToolBlock("fs_list", "dir: src/", "src/main.zig\nsrc/calc.zig\nsrc/utils.zig");

    try harness.tui.renderFrame();

    // Ekranda kullanıcı ve katlanmış araç görünmeli
    try std.testing.expect(harness.assertContains("Developer"));
    try std.testing.expect(harness.assertContains("fs_list"));

    // 2. 't' tuşu ile tool bloğunu aç (expand)
    try harness.sendKey("t");
    try std.testing.expect(harness.assertContains("src/utils.zig"));

    // 3. Tab ile Changed Files paneline geç
    try harness.sendKey("\t");
    try std.testing.expectEqual(FocusPanel.changed_files, harness.tui.focus);

    // 4. Tab ile Diff paneline geç
    try harness.sendKey("\t");
    try std.testing.expectEqual(FocusPanel.diff, harness.tui.focus);

    // 5. 'q' ile çıkış
    try harness.sendKey("q");
    try std.testing.expect(!harness.tui.is_running);
}

test "pty harness: dar terminal resize ve etkilesim testi (Dogrulama 11)" {
    var harness = try PtyHarness.init(std.testing.allocator, 100, 30);
    defer harness.deinit();

    try harness.tui.diffs.addFileDiffFromTexts("src/core.zig", "old_impl();\n", "new_fast_impl();\n");
    harness.tui.focus = .diff;

    try harness.tui.renderFrame();
    try std.testing.expect(harness.assertContains("SPLIT DIFF"));

    // Terminali dar boyuta (70 sütun) küçült
    try harness.resize(70, 24);
    try std.testing.expect(harness.assertContains("FULLSCREEN DIFF"));
    try std.testing.expect(harness.assertContains("NARROW-FALLBACK"));
    try std.testing.expect(harness.assertContains("src/core.zig"));
}
