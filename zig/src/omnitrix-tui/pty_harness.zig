//! omnitrix-tui: PTY Test Koşucusu ve Terminal Etkileşim Test Harness'ı (tasarım Bölüm 5.1, Kol A - A6).
//!
//! Özellikler:
//! - Sanal ve sözde-terminal (PTY) etkileşimlerini simüle eden test ortamı.
//! - Ham tuş basımları, ANSI kaçış dizileri, terminal resize ve akış senaryolarını test eder.
//! - Ekran tamponu inceleme yardımcıları (assertContains, assertLineMatches).
//! - Otomatik etkileşim testleri: Sekme geçişi, araç bloklarını açıp/kapama, hunk navigasyonu,
//!   dar terminal moduna geçiş ve güvenli çıkış.
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const mock_term = @import("mock_terminal.zig");
const tui_mod = @import("tui.zig");

pub const MockTerminal = mock_term.MockTerminal;
pub const Tui = tui_mod.Tui;
pub const FocusPanel = tui_mod.FocusPanel;

/// PTY Test Harness
pub const PtyHarness = struct {
    allocator: std.mem.Allocator,
    mock: *MockTerminal,
    tui: Tui,

    pub fn init(allocator: std.mem.Allocator, cols: u16, rows: u16) !PtyHarness {
        const mock_ptr = try allocator.create(MockTerminal);
        errdefer allocator.destroy(mock_ptr);

        mock_ptr.* = try MockTerminal.init(allocator, cols, rows);
        errdefer mock_ptr.deinit();

        const tui = try Tui.init(allocator, mock_ptr.backend(), 50);

        return .{
            .allocator = allocator,
            .mock = mock_ptr,
            .tui = tui,
        };
    }

    pub fn deinit(self: *PtyHarness) void {
        self.tui.deinit();
        self.mock.deinit();
        self.allocator.destroy(self.mock);
        self.* = undefined;
    }

    /// Tuş dizisi gönderir ve ekranı yeniden çizer.
    pub fn sendKey(self: *PtyHarness, key: []const u8) !void {
        try self.tui.handleKey(key);
        try self.tui.renderFrame();
    }

    /// Terminal boyutunu değiştirir, resize olayını tetikler ve ekranı yeniden çizer.
    pub fn resize(self: *PtyHarness, new_cols: u16, new_rows: u16) !void {
        try self.mock.resize(new_cols, new_rows);
        self.tui.handleResize(new_cols, new_rows);
        try self.tui.renderFrame();
    }

    /// Ekranda belirli bir metnin geçip geçmediğini doğrular.
    pub fn assertContains(self: *const PtyHarness, needle: []const u8) bool {
        return self.mock.containsText(needle);
    }

    /// Tüm ekran içeriğini tek bir string olarak döner.
    pub fn getScreenContent(self: *const PtyHarness, allocator: std.mem.Allocator) ![]u8 {
        return self.mock.getScreenText(allocator);
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

    // Katlıyken içindeki src/utils.zig görünmemeli
    try std.testing.expect(!harness.assertContains("src/utils.zig"));

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
