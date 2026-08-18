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
const sidebar_mod = @import("sidebar.zig");
const question_view_mod = @import("question_view.zig");

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
pub const Sidebar = sidebar_mod.Sidebar;
pub const QuestionViewState = question_view_mod.QuestionViewState;

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
    sidebar: Sidebar,
    question_box: QuestionViewState,

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

        var default_qb = QuestionViewState.init(allocator, "Crush'da olan ama senin kodunda olmayan 3 mekanizma (prompt birleştirme, kesin iptal, döngü tespiti) için hangisini yapalım?", false);
        try default_qb.addOption("Crush'dan al, ekle", "Crush'daki fold + iptal + döngü tespitini alıp Omnitrix'e ekleyeceğiz. Senin kodunda olmayan kısımlar crush'dan tamamlanacak.");
        try default_qb.addOption("Kendi kodundakileri kullan", "Senin kodundaki mekanizmaları (combine, doom_loop sinyali, goal stall) temel alacağız, crush'dan bir şey eklemeyeceğiz.");
        try default_qb.addOption("İkisini birleştir", "İkisinin de en iyi kısımlarını birleştireceğiz: crush'ın kesin iptal mekanizması + senin kodundaki combine ve goal stall.");

        return .{
            .allocator = allocator,
            .backend = backend,
            .blocks = BlockRenderer.init(allocator, max_blocks),
            .diffs = diffs_inst,
            .changed_panel = changed_inst,
            .goal = default_goal,
            .voice = VoiceMode.init(allocator),
            .input_box = InputBox.init(allocator),
            .sidebar = Sidebar.init(allocator),
            .question_box = default_qb,
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
        self.sidebar.deinit();
        self.question_box.deinit();
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

    /// Tüm TUI ekranını OpenCode 1.18.18 stili ile çizer (Screenshot 1:1 Pixel-Perfect Layout)
    pub fn renderFrame(self: *Tui) !void {
        self.voice.tick();

        const width = self.size.cols;
        const height = self.size.rows;
        if (width < 40 or height < 10) return;

        var frame_buf = std.ArrayList(u8).empty;
        defer frame_buf.deinit(self.allocator);

        // İmleci gizle ve ana konuma dön (Ekranı sıfırlamadan sıfır titreşimle üzerine yaz)
        try frame_buf.appendSlice(self.allocator, "\x1b[?25l\x1b[H");

        const max_line_w = if (width > 1) width - 1 else width;

        // 1. ÜST SEKMELER ÇUBUĞU (Top OS Tabs Bar)
        var tab_buf = std.ArrayList(u8).empty;
        defer tab_buf.deinit(self.allocator);

        const tab1_style = if (self.focus == .conversation) Style{ .fg = Color.bright_white, .bg = Color{ .ansi = 237 }, .bold = true } else Style{ .fg = Color{ .ansi = 244 }, .bg = Color{ .ansi = 234 } };
        const tab2_style = if (self.focus == .changed_files) Style{ .fg = Color.bright_white, .bg = Color{ .ansi = 237 }, .bold = true } else Style{ .fg = Color{ .ansi = 244 }, .bg = Color{ .ansi = 234 } };
        const tab3_style = if (self.focus == .diff) Style{ .fg = Color.bright_white, .bg = Color{ .ansi = 237 }, .bold = true } else Style{ .fg = Color{ .ansi = 244 }, .bg = Color{ .ansi = 234 } };

        try term.appendStyle(&tab_buf, self.allocator, tab1_style);
        try tab_buf.appendSlice(self.allocator, "  OC | Omnitrix projesi ilk adım ✕  ");
        try term.appendStyle(&tab_buf, self.allocator, tab2_style);
        try tab_buf.appendSlice(self.allocator, "  agy --dangerously-skip-permissions  ");
        try term.appendStyle(&tab_buf, self.allocator, tab3_style);
        try tab_buf.appendSlice(self.allocator, "  .../Belgeler/omnitrix/zig  ");
        try tab_buf.appendSlice(self.allocator, theme.ANSI.reset);

        try frame_buf.appendSlice(self.allocator, tab_buf.items);
        try frame_buf.appendSlice(self.allocator, "\x1b[K\n");

        // 2. İKİ SÜTUNLU MEKANSAL ALAN (Left: Chat/Question, Right: OpenCode Sidebar)
        const sidebar_w: usize = if (width >= 100) @min(38, width / 3) else 0;
        const main_content_w: usize = if (sidebar_w > 0) max_line_w - sidebar_w - 2 else max_line_w;

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

        // Sağ Sidebar Satırlarını Oluştur
        if (sidebar_w > 0) {
            var sb_lines = try self.sidebar.renderToLines(self.allocator, sidebar_w);
            defer sb_lines.deinit(self.allocator);
            for (sb_lines.items) |sbl| try sidebar_lines.append(self.allocator, sbl);
        }

        // Seçili panele göre ana içerik
        switch (self.focus) {
            .conversation => {
                // Sol Ana İçerik Satırlarını Oluştur
                if (self.voice.state != .off) {
                    var v_lines = try self.voice.renderVoiceBanner(self.allocator, main_content_w);
                    defer v_lines.deinit(self.allocator);
                    for (v_lines.items) |vl| try main_lines.append(self.allocator, vl);
                    try main_lines.append(self.allocator, try self.allocator.dupe(u8, ""));
                }

                // Konuşma metinleri (Markdown dökümü)
                var c_lines = try self.blocks.renderToLines(self.allocator, main_content_w);
                defer c_lines.deinit(self.allocator);
                for (c_lines.items) |cl| try main_lines.append(self.allocator, cl);

                // Durum: Asked 1 question
                var ask_buf = std.ArrayList(u8).empty;
                defer ask_buf.deinit(self.allocator);
                try term.appendStyle(&ask_buf, self.allocator, .{ .fg = Color{ .ansi = 244 } });
                try ask_buf.appendSlice(self.allocator, "→ Asked 1 question");
                try ask_buf.appendSlice(self.allocator, theme.ANSI.reset);
                try main_lines.append(self.allocator, try self.allocator.dupe(u8, ask_buf.items));
                try main_lines.append(self.allocator, try self.allocator.dupe(u8, ""));

                // OpenCode Soru & Seçim Kartı
                var q_lines = try self.question_box.renderToLines(self.allocator, main_content_w);
                defer q_lines.deinit(self.allocator);
                for (q_lines.items) |ql| try main_lines.append(self.allocator, ql);
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

        // Satır Satır Birleştirerek Ekrana Yaz (Zero-Flicker Overwrite)
        const max_body_rows: usize = if (height > 1) height - 1 else 1;
        const start_row = if (main_lines.items.len > max_body_rows) main_lines.items.len - max_body_rows else 0;
        var r_count: usize = 0;

        for (main_lines.items[start_row..]) |line| {
            if (r_count >= max_body_rows) break;

            if (sidebar_w > 0) {
                const truncated_main = try unicode.truncateToWidth(self.allocator, line, main_content_w, "");
                defer self.allocator.free(truncated_main);
                try frame_buf.appendSlice(self.allocator, truncated_main);

                // Ortadaki boşluğu doldur
                const cur_main_w = unicode.strWidth(truncated_main);
                var pad_main = if (main_content_w > cur_main_w) main_content_w - cur_main_w else 0;
                while (pad_main > 0) : (pad_main -= 1) {
                    try frame_buf.appendSlice(self.allocator, " ");
                }

                // Dikey Ayırıcı Çizgi (Muted vertical separator)
                try frame_buf.appendSlice(self.allocator, " │ ");

                // Sağ Sidebar Satırı
                const s_line = if (r_count < sidebar_lines.items.len) sidebar_lines.items[r_count] else "";
                const truncated_side = try unicode.truncateToWidth(self.allocator, s_line, sidebar_w, "");
                defer self.allocator.free(truncated_side);
                try frame_buf.appendSlice(self.allocator, truncated_side);
            } else {
                const truncated_main = try unicode.truncateToWidth(self.allocator, line, max_line_w, "");
                defer self.allocator.free(truncated_main);
                try frame_buf.appendSlice(self.allocator, truncated_main);
            }

            r_count += 1;
            if (r_count < max_body_rows) {
                try frame_buf.appendSlice(self.allocator, "\x1b[K\n");
            } else {
                try frame_buf.appendSlice(self.allocator, "\x1b[K");
            }
        }

        // Kalan boş satırları doldur
        while (r_count < max_body_rows) {
            if (sidebar_w > 0 and r_count < sidebar_lines.items.len) {
                var p: usize = 0;
                while (p < main_content_w) : (p += 1) try frame_buf.appendSlice(self.allocator, " ");
                try frame_buf.appendSlice(self.allocator, " │ ");
                const s_line = sidebar_lines.items[r_count];
                const truncated_side = try unicode.truncateToWidth(self.allocator, s_line, sidebar_w, "");
                defer self.allocator.free(truncated_side);
                try frame_buf.appendSlice(self.allocator, truncated_side);
            }

            r_count += 1;
            if (r_count < max_body_rows) {
                try frame_buf.appendSlice(self.allocator, "\x1b[K\n");
            } else {
                try frame_buf.appendSlice(self.allocator, "\x1b[K");
            }
        }

        // Tek seferde terminale bas (Zero Flicker Atomic Syscall)
        try self.backend.write(frame_buf.items);
        try self.backend.flush();
    }
};

test "opencode 1.18.18 tui layout, sidebar, question box ve voice render" {
    const pty_harness_mod = @import("pty_harness.zig");
    var harness = try pty_harness_mod.PtyHarness.init(std.testing.allocator, 120, 40);
    defer harness.deinit();

    _ = try harness.tui.blocks.addBlock(.user, "Operator", "Sorun şu: Feature matrix'deki mekanizmaları al.");
    _ = try harness.tui.blocks.addBlock(.agent, "Omnitrix", "Crush'daki mekanizmalar ile kendi kodundaki mekanizmaları karşılaştırıyoruz.");

    try harness.tui.renderFrame();

    try std.testing.expect(harness.assertContains("Omnitrix projesi ilk adım"));
    try std.testing.expect(harness.assertContains("Context"));
    try std.testing.expect(harness.assertContains("MCP"));
    try std.testing.expect(harness.assertContains("Build"));
    try std.testing.expect(harness.assertContains("MiMo-V2.5-Pro"));
    try std.testing.expect(harness.assertContains("Crush"));
    try std.testing.expect(harness.assertContains("OpenCode 1.18.18"));

    // Sesli mod aç (Ctrl+V)
    try harness.tui.handleKey("\x16");
    try std.testing.expect(harness.tui.voice.state == .listening);

    try harness.tui.renderFrame();
    try std.testing.expect(harness.assertContains("GROK VOICE MODE"));
}
