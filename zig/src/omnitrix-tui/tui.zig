//! omnitrix-tui: Ana TUI Motoru ve Durum Koordinatörü (tasarım Bölüm 5, 5.1, 5.2, Kol A).
//!
//! Özellikler:
//! - TerminalBackend, BlockRenderer, DiffRenderer ve ChangedFilesPanel bileşenlerini yönetir.
//! - Tek-süreç mimarisinde runtime StateStore'dan snapshot alır veya typed olayları işler.
//! - Odak (Focus) yönetimi: conversation, changed_files, diff.
//! - Ekran düzeni (Layout) hesaplama:
//!   - Geniş ekran (>= 80 cols): Çok panelli veya bölünmüş (split) görünüm.
//!   - Dar ekran (< 80 cols): Otomatik fullscreen diff / fullscreen panel fallback (Bölüm 5.1).
//! - Klavye ve girdi olaylarını işleme (yön tuşları, Tab, Enter, kısayollar).
//! - Deterministik kare çizimi (renderFrame).
//! - Resize sırasında seçili dosya, seçili hunk ve imleç durumunu güvenle korur (Doğrulama 11).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const term = @import("terminal.zig");
const unicode = @import("unicode.zig");
const block_renderer = @import("block_renderer.zig");
const diff_renderer = @import("diff_renderer.zig");
const changed_files = @import("changed_files.zig");

pub const TerminalBackend = term.TerminalBackend;
pub const TerminalSize = term.TerminalSize;
pub const Style = term.Style;
pub const Color = term.Color;
pub const ANSI = term.ANSI;

pub const BlockRenderer = block_renderer.BlockRenderer;
pub const DiffRenderer = diff_renderer.DiffRenderer;
pub const ChangedFilesPanel = changed_files.ChangedFilesPanel;

pub const FocusPanel = enum {
    conversation,
    changed_files,
    diff,
};

pub const Tui = struct {
    allocator: std.mem.Allocator,
    backend: TerminalBackend,
    blocks: BlockRenderer,
    diffs: DiffRenderer,
    changed_panel: ChangedFilesPanel,

    focus: FocusPanel = .conversation,
    size: TerminalSize = .{ .cols = 80, .rows = 24 },
    is_running: bool = true,
    status_message: ?[]const u8 = null,
    model_name: []const u8 = "Omnitrix Engine",
    session_id: []const u8 = "default_session",
    turn_id: u64 = 0,

    pub fn init(
        allocator: std.mem.Allocator,
        backend: TerminalBackend,
        max_blocks: usize,
    ) !Tui {
        const size = backend.getSize() catch TerminalSize{ .cols = 80, .rows = 24 };

        return .{
            .allocator = allocator,
            .backend = backend,
            .blocks = BlockRenderer.init(allocator, max_blocks),
            .diffs = DiffRenderer.init(allocator),
            .changed_panel = ChangedFilesPanel.init(allocator),
            .focus = .conversation,
            .size = size,
            .is_running = true,
            .status_message = null,
            .model_name = "Omnitrix Engine",
            .session_id = "default_session",
            .turn_id = 0,
        };
    }

    pub fn deinit(self: *Tui) void {
        if (self.status_message) |sm| self.allocator.free(sm);
        self.blocks.deinit();
        self.diffs.deinit();
        self.changed_panel.deinit();
        self.* = undefined;
    }

    /// Terminal boyut değişimini işler ve tüm alt bileşenlerin seçimlerini korur (Doğrulama 11).
    pub fn handleResize(self: *Tui, new_cols: u16, new_rows: u16) void {
        self.size = .{ .cols = new_cols, .rows = new_rows };
        self.diffs.handleResize(new_cols, new_rows);
        self.changed_panel.handleResize(new_cols, new_rows);
    }

    /// Odak panelini bir sonraki panele geçirir (Tab).
    pub fn cycleFocus(self: *Tui) void {
        self.focus = switch (self.focus) {
            .conversation => .changed_files,
            .changed_files => .diff,
            .diff => .conversation,
        };
    }

    /// Klavye girdisini işler
    pub fn handleKey(self: *Tui, key: []const u8) !void {
        if (key.len == 0) return;

        // Tab: Odak değiştir
        if (key.len == 1 and key[0] == '\t') {
            self.cycleFocus();
            return;
        }

        // 'q' veya Ctrl+C: Çıkış
        if ((key.len == 1 and key[0] == 'q') or (key.len == 1 and key[0] == 3)) {
            self.is_running = false;
            return;
        }

        // '1', '2', '3': Doğrudan panel seçimi
        if (key.len == 1 and key[0] == '1') {
            self.focus = .conversation;
            return;
        }
        if (key.len == 1 and key[0] == '2') {
            self.focus = .changed_files;
            return;
        }
        if (key.len == 1 and key[0] == '3') {
            self.focus = .diff;
            return;
        }

        // Panel bazlı tuş kontrolleri
        switch (self.focus) {
            .conversation => {
                // 't': En son/aktif tool bloğunu katla/aç
                if (key.len == 1 and key[0] == 't') {
                    if (self.blocks.active_block_id) |bid| {
                        _ = self.blocks.toggleToolCollapse(bid);
                    }
                }
            },
            .changed_files => {
                // Yukarı / Aşağı ok tuşları (\x1b[A / \x1b[B veya 'k' / 'j')
                if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
                    if (self.changed_panel.selected_index > 0) {
                        self.changed_panel.selected_index -= 1;
                    }
                } else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
                    const max_idx = if (self.changed_panel.selected_in_agent)
                        if (self.changed_panel.agent_entries.items.len > 0) self.changed_panel.agent_entries.items.len - 1 else 0
                    else if (self.changed_panel.project_entries.items.len > 0) self.changed_panel.project_entries.items.len - 1 else 0;

                    if (self.changed_panel.selected_index < max_idx) {
                        self.changed_panel.selected_index += 1;
                    }
                } else if (key.len == 1 and key[0] == ' ') {
                    // Space: Agent vs Project listesi arasında geçiş yap
                    self.changed_panel.selected_in_agent = !self.changed_panel.selected_in_agent;
                    self.changed_panel.selected_index = 0;
                }
            },
            .diff => {
                // Diff hunk navigasyonu (j/k veya aşağı/yukarı)
                if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
                    if (self.diffs.selected_hunk_index > 0) {
                        self.diffs.selected_hunk_index -= 1;
                    }
                } else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
                    if (self.diffs.files.items.len > 0) {
                        const cur_f = &self.diffs.files.items[self.diffs.selected_file_index];
                        if (cur_f.hunks.items.len > 0 and self.diffs.selected_hunk_index + 1 < cur_f.hunks.items.len) {
                            self.diffs.selected_hunk_index += 1;
                        }
                    }
                }
            },
        }
    }

    /// Tüm TUI ekranını deterministik olarak çizer
    pub fn renderFrame(self: *Tui) !void {
        try self.backend.clearScreen();
        try self.backend.moveCursor(0, 0);

        var frame_buf = std.ArrayList(u8).empty;
        defer frame_buf.deinit(self.allocator);

        const width = self.size.cols;
        const height = self.size.rows;

        // 1. Üst Başlık ve Sekmeler Satırı (Header Tab bar)
        const is_narrow = self.diffs.isNarrow();
        const tab1 = if (is_narrow)
            (if (self.focus == .conversation) "[1*]" else "[1]")
        else
            (if (self.focus == .conversation) "[1: Conversation *]" else "[1: Conversation]");

        const tab2 = if (is_narrow)
            (if (self.focus == .changed_files) "[2*]" else "[2]")
        else
            (if (self.focus == .changed_files) "[2: Changed Files *]" else "[2: Changed Files]");

        const tab3 = if (is_narrow)
            (if (self.focus == .diff) "[3*]" else "[3]")
        else
            (if (self.focus == .diff) "[3: Diff *]" else "[3: Diff]");

        const narrow_badge = if (is_narrow) " [NARROW-FALLBACK]" else "";
        const title_line = try std.fmt.allocPrint(
            self.allocator,
            "OMNITRIX TUI ⚡ {s} {s} {s}{s}",
            .{ tab1, tab2, tab3, narrow_badge },
        );
        defer self.allocator.free(title_line);

        const truncated_title = try unicode.truncateToWidth(self.allocator, title_line, width, "");
        defer self.allocator.free(truncated_title);

        const padded_title = try unicode.padToWidth(self.allocator, truncated_title, width);
        defer self.allocator.free(padded_title);

        try term.appendStyle(&frame_buf, self.allocator, .{ .fg = Color.bright_white, .bg = Color{ .ansi = 236 }, .bold = true });
        try frame_buf.appendSlice(self.allocator, padded_title);
        try frame_buf.appendSlice(self.allocator, ANSI.reset);
        try frame_buf.appendSlice(self.allocator, "\n");

        // 2. Ana Gövde Render'ı (Odak veya ekran genişliğine göre)
        const body_height: usize = if (height > 3) height - 3 else 1;

        var body_lines = std.ArrayList([]const u8).empty;
        defer {
            for (body_lines.items) |l| self.allocator.free(l);
            body_lines.deinit(self.allocator);
        }

        if (self.diffs.isNarrow() or self.focus != .conversation) {
            // Tek panel görünümü (özellikle dar ekranda veya diff/changed_files odaklıyken)
            switch (self.focus) {
                .conversation => {
                    body_lines = try self.blocks.renderToLines(self.allocator, width);
                },
                .changed_files => {
                    body_lines = try self.changed_panel.renderToLines(self.allocator, width);
                },
                .diff => {
                    body_lines = try self.diffs.renderActiveDiffToLines(self.allocator, width);
                },
            }
        } else {
            // Geniş ekran varsayılan conversation görünümü
            body_lines = try self.blocks.renderToLines(self.allocator, width);
        }

        // Gövde satırlarını ekrana yaz
        var r: usize = 0;
        while (r < body_height) : (r += 1) {
            if (r < body_lines.items.len) {
                const line = body_lines.items[r];
                const truncated = try unicode.truncateToWidth(self.allocator, line, width, "");
                defer self.allocator.free(truncated);
                try frame_buf.appendSlice(self.allocator, truncated);
            }
            try frame_buf.appendSlice(self.allocator, "\n");
        }

        // 3. Alt Durum Çubuğu (Status Bar)
        var stat_buf: [256]u8 = undefined;
        const status_line = try std.fmt.bufPrint(
            &stat_buf,
            " Model: {s} | Session: {s} | Turn: {d} | Tab: Switch | q: Quit",
            .{ self.model_name, self.session_id, self.turn_id },
        );
        const padded_status = try unicode.padToWidth(self.allocator, status_line, width);
        defer self.allocator.free(padded_status);

        try term.appendStyle(&frame_buf, self.allocator, .{ .fg = Color.black, .bg = Color{ .ansi = 250 }, .dim = false });
        try frame_buf.appendSlice(self.allocator, padded_status);
        try frame_buf.appendSlice(self.allocator, ANSI.reset);

        try self.backend.write(frame_buf.items);
        try self.backend.flush();
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11 / A1-A5)
// -----------------------------------------------------------------------------

test "tui motoru baslatma, frame cizimi ve sekme gecisi" {
    const mock_term = @import("mock_terminal.zig");
    var mock = try mock_term.MockTerminal.init(std.testing.allocator, 80, 24);
    defer mock.deinit();

    var app = try Tui.init(std.testing.allocator, mock.backend(), 20);
    defer app.deinit();

    // 1. Blok ekle ve frame çiz
    _ = try app.blocks.addBlock(.user, "User", "Hello Omnitrix");
    try app.renderFrame();

    try std.testing.expect(mock.containsText("OMNITRIX TUI"));
    try std.testing.expect(mock.containsText("Hello Omnitrix"));

    // 2. Tab ile odak değiştir
    try app.handleKey("\t");
    try std.testing.expectEqual(FocusPanel.changed_files, app.focus);

    try app.handleKey("\t");
    try std.testing.expectEqual(FocusPanel.diff, app.focus);

    try app.handleKey("\t");
    try std.testing.expectEqual(FocusPanel.conversation, app.focus);
}

test "tui dar ekran resize ve fallback cizimi (Dogrulama 11)" {
    const mock_term = @import("mock_terminal.zig");
    var mock = try mock_term.MockTerminal.init(std.testing.allocator, 80, 24);
    defer mock.deinit();

    var app = try Tui.init(std.testing.allocator, mock.backend(), 20);
    defer app.deinit();

    // Diff ekle
    try app.diffs.addFileDiffFromTexts("src/test.zig", "a = 1;\n", "a = 2;\n");
    app.focus = .diff;

    // Dar ekrana küçült (75 sütun)
    try mock.resize(75, 20);
    app.handleResize(75, 20);

    try app.renderFrame();

    try std.testing.expect(app.diffs.isNarrow());
    try std.testing.expect(mock.containsText("NARROW-FALLBACK"));
    try std.testing.expect(mock.containsText("src/test.zig"));
}
