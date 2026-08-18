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

    /// Tüm TUI ekranını OpenCode & Grok stili ile çizer (Zero-Flicker Spatial Render)
    pub fn renderFrame(self: *Tui) !void {
        self.voice.tick();

        const width = self.size.cols;
        const height = self.size.rows;
        if (width < 30 or height < 10) return;

        var frame_buf = std.ArrayList(u8).empty;
        defer frame_buf.deinit(self.allocator);

        // İmleci gizle ve ana konuma dön (Ekranı sıfırlamadan sıfır titreşimle üzerine yaz)
        try frame_buf.appendSlice(self.allocator, "\x1b[?25l\x1b[H");

        const max_line_w = if (width > 1) width - 1 else width;

        // 1. ÜST BAŞLIK ÇUBUĞU (Row 0)
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

        var h_pad = if (max_line_w > head_content_w) max_line_w - head_content_w else 0;
        try term.appendStyle(&head_buf, self.allocator, .{ .bg = theme.Theme.header_bg });
        while (h_pad > 0) : (h_pad -= 1) {
            try head_buf.appendSlice(self.allocator, " ");
        }

        try term.appendStyle(&head_buf, self.allocator, .{ .fg = theme.Theme.text_green, .bg = theme.Theme.header_bg, .bold = true });
        try head_buf.appendSlice(self.allocator, right_badge);
        try head_buf.appendSlice(self.allocator, theme.ANSI.reset);

        try frame_buf.appendSlice(self.allocator, head_buf.items);
        try frame_buf.appendSlice(self.allocator, "\x1b[K\n");

        // 2. GÖVDE VE İÇERİK HESAPLAMA (Spatial Multi-Panel)
        const is_spatial_split = (width >= 105 and self.focus == .conversation);
        const sidebar_w: usize = if (is_spatial_split) 30 else 0;
        const main_content_w: usize = if (is_spatial_split) max_line_w - sidebar_w - 1 else max_line_w;

        var main_lines = std.ArrayList([]const u8).empty;
        defer {
            for (main_lines.items) |l| self.allocator.free(l);
            main_lines.deinit(self.allocator);
        }

        var sidebar_lines = std.ArrayList([]const u8).empty;
        defer {
            for (sidebar_lines.items) |l| self.allocator.free(l);
            sidebar_lines.deinit(self.allocator);
        }

        if (is_spatial_split) {
            // Sol Yan Panel: Workspace & Subagents
            var side_buf = std.ArrayList(u8).empty;
            defer side_buf.deinit(self.allocator);

            // Başlık
            try term.appendStyle(&side_buf, self.allocator, .{ .fg = theme.Theme.text_cyan, .bold = true });
            try side_buf.appendSlice(self.allocator, "╭─ 📂 Workspace ─────────╮");
            try side_buf.appendSlice(self.allocator, theme.ANSI.reset);
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, side_buf.items));
            side_buf.clearRetainingCapacity();

            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│ 🌿 masterplan          │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│ 📁 src/                │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│   📄 root.zig          │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│   📁 omnitrix-tui/     │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│   📁 omnitrix-task/    │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "├─ 🤖 Subagents ─────────┤"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│  ◆ Kaşif       [idle]  │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│  ◆ Juryrigg    [run]   │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "│  ◆ Bal Porsuğu [idle]  │"));
            try sidebar_lines.append(self.allocator, try self.allocator.dupe(u8, "╰────────────────────────╯"));
        }

        // Voice banner aktifse ekle
        if (self.voice.state != .off) {
            var v_lines = try self.voice.renderVoiceBanner(self.allocator, main_content_w);
            defer v_lines.deinit(self.allocator);
            for (v_lines.items) |vl| try main_lines.append(self.allocator, vl);
        }

        // Goal tracker aktifse ekle
        if (self.focus == .conversation) {
            var g_lines = try self.goal.renderToLines(self.allocator, main_content_w);
            defer g_lines.deinit(self.allocator);
            for (g_lines.items) |gl| try main_lines.append(self.allocator, gl);
        }

        // Seçili panele göre ana içerik
        switch (self.focus) {
            .conversation => {
                var c_lines = try self.blocks.renderToLines(self.allocator, main_content_w);
                defer c_lines.deinit(self.allocator);
                for (c_lines.items) |cl| try main_lines.append(self.allocator, cl);
            },
            .changed_files => {
                var f_lines = try self.changed_panel.renderToLines(self.allocator, max_line_w);
                defer f_lines.deinit(self.allocator);
                for (f_lines.items) |fl| try main_lines.append(self.allocator, fl);
            },
            .diff => {
                var d_lines = try self.diffs.renderActiveDiffToLines(self.allocator, max_line_w);
                defer d_lines.deinit(self.allocator);
                for (d_lines.items) |dl| try main_lines.append(self.allocator, dl);
            },
        }

        // 3. ALT GİRİŞ KUTUSU VE KISAYOLLAR
        var input_lines = try self.input_box.renderToLines(self.allocator, max_line_w);
        defer {
            for (input_lines.items) |il| self.allocator.free(il);
            input_lines.deinit(self.allocator);
        }

        const reserved_bottom_rows = input_lines.items.len + 1; // input_box + footer
        const max_body_rows: usize = if (height > reserved_bottom_rows + 1) height - reserved_bottom_rows - 1 else 1;

        // Gövde satırlarını ekrana yaz (Row 1 .. max_body_rows)
        const start_row = if (main_lines.items.len > max_body_rows) main_lines.items.len - max_body_rows else 0;
        var r_count: usize = 0;

        for (main_lines.items[start_row..]) |line| {
            if (r_count >= max_body_rows) break;

            if (is_spatial_split) {
                const s_line = if (r_count < sidebar_lines.items.len) sidebar_lines.items[r_count] else "";
                const s_trunc = try unicode.truncateToWidth(self.allocator, s_line, sidebar_w, "");
                defer self.allocator.free(s_trunc);

                var pad_s = if (sidebar_w > unicode.strWidth(s_trunc)) sidebar_w - unicode.strWidth(s_trunc) else 0;

                try frame_buf.appendSlice(self.allocator, s_trunc);
                while (pad_s > 0) : (pad_s -= 1) {
                    try frame_buf.appendSlice(self.allocator, " ");
                }
                try frame_buf.appendSlice(self.allocator, " ");

                const truncated = try unicode.truncateToWidth(self.allocator, line, main_content_w, "");
                defer self.allocator.free(truncated);
                try frame_buf.appendSlice(self.allocator, truncated);
            } else {
                const truncated = try unicode.truncateToWidth(self.allocator, line, max_line_w, "");
                defer self.allocator.free(truncated);
                try frame_buf.appendSlice(self.allocator, truncated);
            }

            try frame_buf.appendSlice(self.allocator, "\x1b[K\n");
            r_count += 1;
        }

        // Kalan boşlukları temiz satırlarla doldur
        while (r_count < max_body_rows) : (r_count += 1) {
            if (is_spatial_split and r_count < sidebar_lines.items.len) {
                const s_line = sidebar_lines.items[r_count];
                const s_trunc = try unicode.truncateToWidth(self.allocator, s_line, sidebar_w, "");
                defer self.allocator.free(s_trunc);
                try frame_buf.appendSlice(self.allocator, s_trunc);
            }
            try frame_buf.appendSlice(self.allocator, "\x1b[K\n");
        }

        // 4. GİRİŞ KUTUSUNU YAZ
        for (input_lines.items) |iline| {
            const truncated_in = try unicode.truncateToWidth(self.allocator, iline, max_line_w, "");
            defer self.allocator.free(truncated_in);
            try frame_buf.appendSlice(self.allocator, truncated_in);
            try frame_buf.appendSlice(self.allocator, "\x1b[K\n");
        }

        // 5. EN ALT KISAYOL ÇUBUĞU (Row height - 1)
        var foot_buf = std.ArrayList(u8).empty;
        defer foot_buf.deinit(self.allocator);

        try term.appendStyle(&foot_buf, self.allocator, .{ .fg = Color{ .ansi = 250 }, .bg = theme.Theme.status_bg });
        try foot_buf.appendSlice(self.allocator, " [Enter] Send │ [Tab] Panel │ [/] Commands │ [Ctrl+V] Voice │ [q] Quit");

        const foot_content_w = unicode.strWidth(" [Enter] Send │ [Tab] Panel │ [/] Commands │ [Ctrl+V] Voice │ [q] Quit");
        var f_pad = if (max_line_w > foot_content_w) max_line_w - foot_content_w else 0;
        while (f_pad > 0) : (f_pad -= 1) {
            try foot_buf.appendSlice(self.allocator, " ");
        }
        try foot_buf.appendSlice(self.allocator, theme.ANSI.reset);
        try frame_buf.appendSlice(self.allocator, foot_buf.items);
        try frame_buf.appendSlice(self.allocator, "\x1b[K");

        // Tek seferde ekrana bas (Zero Flicker Atomic Write)
        try self.backend.write(frame_buf.items);
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
