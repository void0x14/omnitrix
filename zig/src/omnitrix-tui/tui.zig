//! omnitrix-tui: OpenCode & Grok Stili Ana TUI Motoru (Full Fidelity Terminal UI)
//!
//! Şartname & Vizyon:
//! - OpenCode TUI yerleşimi: Header rozeti, kart tabanlı sohbet, interaktif prompt textarea, slash komut paleti.
//! - Grok CLI Sesli Mod: `Ctrl+V` veya `/voice` ile canlı ses dalgası (equalizer) banner'ı.
//! - Codex & Claude Code Hedef Takibi: Canlı çok adımlı `GoalTracker` ilerleme kutusu.
//! - Changed Files & Diff Panelleri: Git X/Y, +add -del, hunk diff navigasyonu.
//! - Kesintisiz klavye navigasyonu ve canlı rendering.

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");
const block_renderer = @import("block_renderer.zig");
const diff_renderer = @import("diff_renderer.zig");
const changed_files = @import("changed_files.zig");
const goal_mod = @import("goal.zig");
const voice_mod = @import("voice.zig");
const input_box_mod = @import("input_box.zig");

pub const TerminalBackend = term.TerminalBackend;
pub const TerminalSize = term.TerminalSize;
pub const Style = term.Style;
pub const Color = term.Color;
pub const ANSI = term.ANSI;

pub const BlockRenderer = block_renderer.BlockRenderer;
pub const DiffRenderer = diff_renderer.DiffRenderer;
pub const ChangedFilesPanel = changed_files.ChangedFilesPanel;
pub const GoalTracker = goal_mod.GoalTracker;
pub const VoiceMode = voice_mod.VoiceMode;
pub const InputBox = input_box_mod.InputBox;

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
    goal: GoalTracker,
    voice: VoiceMode,
    input_box: InputBox,

    focus: FocusPanel = .conversation,
    size: TerminalSize = .{ .cols = 100, .rows = 30 },
    is_running: bool = true,
    branch_name: []const u8 = "masterplan",
    session_id: []const u8 = "omni_01",
    turn_id: u64 = 1,

    pub fn init(
        allocator: std.mem.Allocator,
        backend: TerminalBackend,
        max_blocks: usize,
    ) !Tui {
        const size = backend.getSize() catch TerminalSize{ .cols = 100, .rows = 30 };

        var default_goal = try GoalTracker.init(allocator, "Omnitrix Single-Process Autonomous Architecture");
        errdefer default_goal.deinit();

        try default_goal.addStep("Step 1: Kernel EventLoop & Monotonic Timers (omnitrix-io)", .completed);
        try default_goal.addStep("Step 2: Task State Machine & Leak Watchdog (omnitrix-task)", .completed);
        try default_goal.addStep("Step 3: Permission Broker & 7-Step Safe Revert (ledger)", .completed);
        try default_goal.addStep("Step 4: OpenCode TUI + Grok Voice Mode Engine (omnitrix-tui)", .active);
        try default_goal.addStep("Step 5: Multi-Agent Subtask & Codebase Memory Graph", .pending);

        var diffs_inst = DiffRenderer.init(allocator);
        diffs_inst.handleResize(size.cols, size.rows);

        var changed_inst = ChangedFilesPanel.init(allocator);
        changed_inst.handleResize(size.cols, size.rows);

        return .{
            .allocator = allocator,
            .backend = backend,
            .blocks = BlockRenderer.init(allocator, max_blocks),
            .diffs = diffs_inst,
            .changed_panel = changed_inst,
            .goal = default_goal,
            .voice = VoiceMode.init(allocator),
            .input_box = InputBox.init(allocator),
            .focus = .conversation,
            .size = size,
            .is_running = true,
            .branch_name = "masterplan",
            .session_id = "omni_01",
            .turn_id = 1,
        };
    }

    pub fn deinit(self: *Tui) void {
        self.blocks.deinit();
        self.diffs.deinit();
        self.changed_panel.deinit();
        self.goal.deinit();
        self.voice.deinit();
        self.input_box.deinit();
        self.* = undefined;
    }

    pub fn handleResize(self: *Tui, new_cols: u16, new_rows: u16) void {
        self.size = .{ .cols = new_cols, .rows = new_rows };
        self.diffs.handleResize(new_cols, new_rows);
        self.changed_panel.handleResize(new_cols, new_rows);
    }

    pub fn cycleFocus(self: *Tui) void {
        self.focus = switch (self.focus) {
            .conversation => .changed_files,
            .changed_files => .diff,
            .diff => .conversation,
        };
    }

    pub fn handleKey(self: *Tui, key: []const u8) !void {
        if (key.len == 0) return;

        // Ctrl+V: Sesli Modu (Voice Mode) aç/kapat
        if (key.len == 1 and key[0] == 22) { // 22 = ASCII Ctrl+V
            self.voice.toggle();
            self.input_box.voice_active = (self.voice.state != .off);
            return;
        }

        // Ctrl+C: Çıkış
        if (key.len == 1 and key[0] == 3) {
            self.is_running = false;
            return;
        }

        // Tab: Odak panelini değiştir
        if (key.len == 1 and key[0] == '\t') {
            self.cycleFocus();
            return;
        }

        // Eğer input box'a odaklanmışsa veya conversation modundaysa tuşları input_box'a ver
        if (self.focus == .conversation) {
            // 't' tuşu ile aktif tool bloğunu aç/kapa
            if (key.len == 1 and key[0] == 't' and self.input_box.text.items.len == 0) {
                if (self.blocks.active_block_id) |ab_id| {
                    _ = self.blocks.toggleToolCollapse(ab_id);
                    return;
                }
            }

            // 'q' sadece input_box boşken çıkış yapsın
            if (key.len == 1 and key[0] == 'q' and self.input_box.text.items.len == 0) {
                self.is_running = false;
                return;
            }

            // Enter tuşu: Mesajı gönder
            if (key.len == 1 and (key[0] == '\r' or key[0] == '\n')) {
                if (try self.input_box.submit()) |submitted_text| {
                    defer self.allocator.free(submitted_text);

                    // Slash komutları kontrol et
                    if (std.mem.eql(u8, submitted_text, "/voice")) {
                        self.voice.toggle();
                        self.input_box.voice_active = (self.voice.state != .off);
                    } else if (std.mem.eql(u8, submitted_text, "/goal")) {
                        self.goal.is_collapsed = !self.goal.is_collapsed;
                    } else if (std.mem.eql(u8, submitted_text, "/diff")) {
                        self.focus = .diff;
                    } else if (std.mem.eql(u8, submitted_text, "/files")) {
                        self.focus = .changed_files;
                    } else if (std.mem.eql(u8, submitted_text, "/clear")) {
                        self.blocks.deinit();
                        self.blocks = BlockRenderer.init(self.allocator, 200);
                    } else {
                        // Normal kullanıcı mesajı ekle
                        _ = try self.blocks.addBlock(.user, "Operator", submitted_text);
                        // Ajan simülasyon yanıtı ekle
                        _ = try self.blocks.addBlock(.agent, "Omnitrix", "Komut alındı ve StateStore üzerinde yürütülüyor.");
                    }
                    return;
                }
            }

            const handled = try self.input_box.handleKey(key);
            if (handled) return;
        }

        // Diğer paneller için yön tuşları
        switch (self.focus) {
            .changed_files => {
                if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
                    if (self.changed_panel.selected_index > 0) self.changed_panel.selected_index -= 1;
                } else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
                    const max_idx = if (self.changed_panel.selected_in_agent)
                        if (self.changed_panel.agent_entries.items.len > 0) self.changed_panel.agent_entries.items.len - 1 else 0
                    else if (self.changed_panel.project_entries.items.len > 0) self.changed_panel.project_entries.items.len - 1 else 0;

                    if (self.changed_panel.selected_index < max_idx) self.changed_panel.selected_index += 1;
                } else if (key.len == 1 and key[0] == ' ') {
                    self.changed_panel.selected_in_agent = !self.changed_panel.selected_in_agent;
                    self.changed_panel.selected_index = 0;
                } else if (key.len == 1 and key[0] == 'q') {
                    self.is_running = false;
                } else if (key.len == 1 and key[0] == 27) { // Escape
                    self.focus = .conversation;
                }
            },
            .diff => {
                if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
                    if (self.diffs.selected_hunk_index > 0) self.diffs.selected_hunk_index -= 1;
                } else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
                    if (self.diffs.files.items.len > 0) {
                        const cur_f = &self.diffs.files.items[self.diffs.selected_file_index];
                        if (cur_f.hunks.items.len > 0 and self.diffs.selected_hunk_index + 1 < cur_f.hunks.items.len) {
                            self.diffs.selected_hunk_index += 1;
                        }
                    }
                } else if (key.len == 1 and key[0] == 'q') {
                    self.is_running = false;
                } else if (key.len == 1 and key[0] == 27) { // Escape
                    self.focus = .conversation;
                }
            },
            .conversation => {},
        }
    }

    /// Tüm TUI ekranını OpenCode & Grok stili ile çizer (Pixel-Perfect Cursor Anchored Render)
    pub fn renderFrame(self: *Tui) !void {
        self.voice.tick();

        try self.backend.clearScreen();

        const width = self.size.cols;
        const height = self.size.rows;
        if (width < 30 or height < 10) return;

        // 1. ÜST BAŞLIK ÇUBUĞU (Row 0)
        try self.backend.moveCursor(0, 0);
        var head_buf = std.ArrayList(u8).empty;
        defer head_buf.deinit(self.allocator);

        try term.appendStyle(&head_buf, self.allocator, .{ .fg = theme.Theme.text_cyan, .bg = theme.Theme.header_bg, .bold = true });
        try head_buf.appendSlice(self.allocator, " ✦ OMNITRIX ");
        try term.appendStyle(&head_buf, self.allocator, .{ .fg = theme.Theme.text_dim, .bg = theme.Theme.header_bg });
        try head_buf.appendSlice(self.allocator, "│ ");

        // Git Branch
        try term.appendStyle(&head_buf, self.allocator, .{ .fg = theme.Theme.text_green, .bg = theme.Theme.header_bg, .bold = true });
        try head_buf.appendSlice(self.allocator, "🌿 ");
        try head_buf.appendSlice(self.allocator, self.branch_name);
        try term.appendStyle(&head_buf, self.allocator, .{ .fg = theme.Theme.text_dim, .bg = theme.Theme.header_bg });
        try head_buf.appendSlice(self.allocator, " │ ");

        // Sekmeler (Tabs)
        const tab1_style = if (self.focus == .conversation) Style{ .fg = Color.bright_white, .bg = Color{ .ansi = 239 }, .bold = true } else Style{ .fg = theme.Theme.text_dim, .bg = theme.Theme.header_bg };
        const tab2_style = if (self.focus == .changed_files) Style{ .fg = Color.bright_white, .bg = Color{ .ansi = 239 }, .bold = true } else Style{ .fg = theme.Theme.text_dim, .bg = theme.Theme.header_bg };
        const tab3_style = if (self.focus == .diff) Style{ .fg = Color.bright_white, .bg = Color{ .ansi = 239 }, .bold = true } else Style{ .fg = theme.Theme.text_dim, .bg = theme.Theme.header_bg };

        try term.appendStyle(&head_buf, self.allocator, tab1_style);
        try head_buf.appendSlice(self.allocator, " [1: Chat] ");

        try term.appendStyle(&head_buf, self.allocator, tab2_style);
        try head_buf.appendSlice(self.allocator, " [2: Files] ");

        try term.appendStyle(&head_buf, self.allocator, tab3_style);
        try head_buf.appendSlice(self.allocator, " [3: Diff] ");

        // Sağ Durum Rozeti
        const right_badge = " 🟢 READY │ single-process ";
        const head_content_w = unicode.strWidth(" ✦ OMNITRIX │ 🌿 ") + unicode.strWidth(self.branch_name) + unicode.strWidth(" │  [1: Chat]  [2: Files]  [3: Diff] ") + unicode.strWidth(right_badge);

        var h_pad = if (width > head_content_w) width - head_content_w else 1;
        try term.appendStyle(&head_buf, self.allocator, .{ .bg = theme.Theme.header_bg });
        while (h_pad > 0) : (h_pad -= 1) {
            try head_buf.appendSlice(self.allocator, " ");
        }

        try term.appendStyle(&head_buf, self.allocator, .{ .fg = theme.Theme.text_green, .bg = theme.Theme.header_bg, .bold = true });
        try head_buf.appendSlice(self.allocator, right_badge);
        try head_buf.appendSlice(self.allocator, theme.ANSI.reset);

        try self.backend.write(head_buf.items);

        // 2. GÖVDE VE İÇERİK HESAPLAMA
        var body_lines = std.ArrayList([]const u8).empty;
        defer {
            for (body_lines.items) |l| self.allocator.free(l);
            body_lines.deinit(self.allocator);
        }

        // Voice banner aktifse en üste ekle
        if (self.voice.state != .off) {
            var v_lines = try self.voice.renderVoiceBanner(self.allocator, width);
            defer v_lines.deinit(self.allocator);
            for (v_lines.items) |vl| try body_lines.append(self.allocator, vl);
            try body_lines.append(self.allocator, try self.allocator.dupe(u8, ""));
        }

        // Goal tracker aktifse ekle
        if (self.focus == .conversation) {
            var g_lines = try self.goal.renderToLines(self.allocator, width);
            defer g_lines.deinit(self.allocator);
            for (g_lines.items) |gl| try body_lines.append(self.allocator, gl);
            try body_lines.append(self.allocator, try self.allocator.dupe(u8, ""));
        }

        // Seçili panele göre ana içerik
        switch (self.focus) {
            .conversation => {
                var c_lines = try self.blocks.renderToLines(self.allocator, width);
                defer c_lines.deinit(self.allocator);
                for (c_lines.items) |cl| try body_lines.append(self.allocator, cl);
            },
            .changed_files => {
                var f_lines = try self.changed_panel.renderToLines(self.allocator, width);
                defer f_lines.deinit(self.allocator);
                for (f_lines.items) |fl| try body_lines.append(self.allocator, fl);
            },
            .diff => {
                var d_lines = try self.diffs.renderActiveDiffToLines(self.allocator, width);
                defer d_lines.deinit(self.allocator);
                for (d_lines.items) |dl| try body_lines.append(self.allocator, dl);
            },
        }

        // 3. ALT GİRİŞ KUTUSU VE KISAYOLLAR
        var input_lines = try self.input_box.renderToLines(self.allocator, width);
        defer {
            for (input_lines.items) |il| self.allocator.free(il);
            input_lines.deinit(self.allocator);
        }

        const reserved_bottom_rows = input_lines.items.len + 1; // input_box + footer
        const max_body_rows: usize = if (height > reserved_bottom_rows + 1) height - reserved_bottom_rows - 1 else 1;

        // Gövde satırlarını ekrana yaz (Row 1 .. max_body_rows)
        const start_row = if (body_lines.items.len > max_body_rows) body_lines.items.len - max_body_rows else 0;
        var r: u16 = 1;

        for (body_lines.items[start_row..]) |line| {
            if (r > max_body_rows) break;
            try self.backend.moveCursor(r, 0);
            try self.backend.write(line);
            r += 1;
        }

        // 4. GİRİŞ KUTUSUNU YERLEŞTİR
        var in_r: u16 = @intCast(height - reserved_bottom_rows);
        for (input_lines.items) |iline| {
            if (in_r >= height - 1) break;
            try self.backend.moveCursor(in_r, 0);
            try self.backend.write(iline);
            in_r += 1;
        }

        // 5. EN ALT KISAYOL ÇUBUĞU (Row height - 1)
        try self.backend.moveCursor(height - 1, 0);
        var foot_buf = std.ArrayList(u8).empty;
        defer foot_buf.deinit(self.allocator);

        try term.appendStyle(&foot_buf, self.allocator, .{ .fg = Color{ .ansi = 250 }, .bg = theme.Theme.status_bg });
        try foot_buf.appendSlice(self.allocator, " [Enter] Send │ [Tab] Panel │ [/] Commands │ [Ctrl+V] Voice │ [q] Quit");

        const foot_content_w = unicode.strWidth(" [Enter] Send │ [Tab] Panel │ [/] Commands │ [Ctrl+V] Voice │ [q] Quit");
        var f_pad = if (width > foot_content_w) width - foot_content_w else 1;
        while (f_pad > 0) : (f_pad -= 1) {
            try foot_buf.appendSlice(self.allocator, " ");
        }
        try foot_buf.appendSlice(self.allocator, theme.ANSI.reset);
        try self.backend.write(foot_buf.items);

        try self.backend.flush();
    }
};

test "opencode tui header, goal, input box ve voice render" {
    const mock_term = @import("mock_terminal.zig");
    var mock = try mock_term.MockTerminal.init(std.testing.allocator, 100, 30);
    defer mock.deinit();

    var app = try Tui.init(std.testing.allocator, mock.backend(), 20);
    defer app.deinit();

    _ = try app.blocks.addBlock(.user, "Operator", "Run full system diagnostic");
    _ = try app.blocks.addBlock(.agent, "Omnitrix", "Diagnostic complete. All 51 core modules nominal.");

    try app.renderFrame();

    try std.testing.expect(mock.containsText("OMNITRIX"));
    try std.testing.expect(mock.containsText("Goal:"));
    try std.testing.expect(mock.containsText("Prompt"));
    try std.testing.expect(mock.containsText("Claude 3.5"));

    // Sesli mod aç
    try app.handleKey("\x16"); // Ctrl+V
    try std.testing.expect(app.voice.state == .listening);

    try app.renderFrame();
    try std.testing.expect(mock.containsText("GROK VOICE MODE"));
}
