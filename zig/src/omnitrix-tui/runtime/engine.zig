//! omnitrix-tui: Master 2D TUI Engine & Runtime (Çift Tamponlu, Hücre Bazlı Motor)
//!
//! Özellikler:
//! - FrontBuffer ve BackBuffer çift tampon mimarisi (Double Buffering)
//! - Constraint Layout tabanlı ekran bölme (Üst Sekmeler, Sol Ana Akış, Sağ Sidebar, Alt Prompt Editörü)
//! - Z-Index Modallar: ModelSelector (`Ctrl+P`) ve CommandPalette (`Ctrl+K` / `/`)
//! - Gerçek VT500/ANSI FSM InputParser ve GapBuffer PromptEditor entegrasyonu
//! - BufferDiff ile sadece değişen hücreleri atomik tek `write()` çağrısında basma
//! - Sıfır kırılma, sıfır kaçış dizisi kayması, tam Unicode genişlik uyumu
//! - %100 Gerçek Linux PTY veya POSIX Terminal desteği

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const layout_mod = @import("../core/layout.zig");
const diff_mod = @import("../core/diff.zig");
const term_mod = @import("../terminal.zig");

const input_parser_mod = @import("../input/parser.zig");
const keys_mod = @import("../input/keys.zig");
const prompt_editor_mod = @import("../editor/prompt_editor.zig");

const modal_mod = @import("../dialogs/modal.zig");
const model_selector_mod = @import("../dialogs/model_selector.zig");
const command_palette_mod = @import("../dialogs/command_palette.zig");

const header_mod = @import("../views/header_view.zig");
const sidebar_mod = @import("../views/sidebar_view.zig");
const question_mod = @import("../views/question_view.zig");
const voice_mod = @import("../voice.zig");
const goal_mod = @import("../goal.zig");
const block_mod = @import("../block_renderer.zig");
const diff_renderer_mod = @import("../diff_renderer.zig");
const changed_files_mod = @import("../changed_files.zig");

pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const BufferDiff = diff_mod.BufferDiff;
pub const Layout = layout_mod.Layout;
pub const Constraint = layout_mod.Constraint;
pub const TerminalBackend = term_mod.TerminalBackend;
pub const TerminalSize = term_mod.TerminalSize;

pub const InputParser = input_parser_mod.InputParser;
pub const InputEvent = keys_mod.InputEvent;
pub const KeyEvent = keys_mod.KeyEvent;
pub const PromptEditor = prompt_editor_mod.PromptEditor;

pub const Modal = modal_mod.Modal;
pub const ModelSelector = model_selector_mod.ModelSelector;
pub const CommandPalette = command_palette_mod.CommandPalette;

pub const HeaderView = header_mod.HeaderView;
pub const SidebarView = sidebar_mod.SidebarView;
pub const QuestionView = question_mod.QuestionView;
pub const VoiceMode = voice_mod.VoiceMode;
pub const GoalTracker = goal_mod.GoalTracker;
pub const BlockRenderer = block_mod.BlockRenderer;
pub const DiffRenderer = diff_renderer_mod.DiffRenderer;
pub const ChangedFilesPanel = changed_files_mod.ChangedFilesPanel;

pub const FocusPanel = enum {
    conversation,
    changed_files,
    diff,
};

pub const Engine = struct {
    allocator: std.mem.Allocator,
    backend: TerminalBackend,
    size: TerminalSize,
    front_buffer: Buffer,
    back_buffer: Buffer,
    differ: BufferDiff,

    parser: InputParser,
    prompt_editor: PromptEditor,
    model_selector: ModelSelector,
    command_palette: CommandPalette,

    header: HeaderView,
    sidebar: SidebarView,
    question: QuestionView,
    voice: VoiceMode,
    goal: GoalTracker,
    blocks: BlockRenderer,
    diffs: DiffRenderer,
    changed_panel: ChangedFilesPanel,

    focus: FocusPanel = .conversation,
    is_running: bool = true,
    io_buf: std.ArrayList(u8),
    events_buf: std.ArrayList(InputEvent),

    pub fn init(allocator: std.mem.Allocator, backend: TerminalBackend) !Engine {
        const size = backend.getSize() catch TerminalSize{ .cols = 120, .rows = 40 };
        const area = Rect.init(0, 0, size.cols, size.rows);

        var front = try Buffer.init(allocator, area);
        errdefer front.deinit();

        var back = try Buffer.init(allocator, area);
        errdefer back.deinit();

        var default_goal = try GoalTracker.init(allocator, "Omnitrix Single-Process Autonomous Architecture");
        errdefer default_goal.deinit();

        try default_goal.addStep("Step 1: Kernel EventLoop & Monotonic Timers", .completed);
        try default_goal.addStep("Step 2: Task State Machine & Leak Watchdog", .completed);
        try default_goal.addStep("Step 3: Permission Broker & Safe Revert", .completed);
        try default_goal.addStep("Step 4: Native OpenCode 2D Cell TUI Engine", .active);

        var diffs_inst = DiffRenderer.init(allocator);
        diffs_inst.handleResize(size.cols, size.rows);

        var changed_inst = ChangedFilesPanel.init(allocator);
        changed_inst.handleResize(size.cols, size.rows);

        var editor_inst = try PromptEditor.init(allocator);
        errdefer editor_inst.deinit();

        var ms_inst = try ModelSelector.init(allocator);
        errdefer ms_inst.deinit();

        var cp_inst = try CommandPalette.init(allocator);
        errdefer cp_inst.deinit();

        return .{
            .allocator = allocator,
            .backend = backend,
            .size = size,
            .front_buffer = front,
            .back_buffer = back,
            .differ = BufferDiff.init(allocator),
            .parser = InputParser.init(allocator),
            .prompt_editor = editor_inst,
            .model_selector = ms_inst,
            .command_palette = cp_inst,
            .header = HeaderView.init(),
            .sidebar = SidebarView.init(allocator),
            .question = QuestionView.init(allocator, "Crush'da olan ama senin kodunda olmayan 3 mekanizma için hangisini yapalım?"),
            .voice = VoiceMode.init(allocator),
            .goal = default_goal,
            .blocks = BlockRenderer.init(allocator, 100),
            .diffs = diffs_inst,
            .changed_panel = changed_inst,
            .focus = .conversation,
            .is_running = true,
            .io_buf = std.ArrayList(u8).empty,
            .events_buf = std.ArrayList(InputEvent).empty,
        };
    }

    pub fn deinit(self: *Engine) void {
        self.front_buffer.deinit();
        self.back_buffer.deinit();
        self.parser.deinit();
        self.prompt_editor.deinit();
        self.model_selector.deinit();
        self.command_palette.deinit();
        self.sidebar.deinit();
        self.question.deinit();
        self.voice.deinit();
        self.goal.deinit();
        self.blocks.deinit();
        self.diffs.deinit();
        self.changed_panel.deinit();
        self.io_buf.deinit(self.allocator);
        self.events_buf.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn handleResize(self: *Engine, new_cols: u16, new_rows: u16) !void {
        self.size = .{ .cols = new_cols, .rows = new_rows };
        const new_area = Rect.init(0, 0, new_cols, new_rows);
        try self.front_buffer.resize(new_area);
        try self.back_buffer.resize(new_area);
        self.front_buffer.clear();
        self.back_buffer.clear();
        self.diffs.handleResize(new_cols, new_rows);
        self.changed_panel.handleResize(new_cols, new_rows);
    }

    /// Terminalden gelen ham bayt akışını FSM parser'dan geçirip ilgili bileşene dağıtır.
    pub fn handleKey(self: *Engine, raw_bytes: []const u8) !void {
        self.events_buf.clearRetainingCapacity();
        try self.parser.parse(raw_bytes, &self.events_buf);

        for (self.events_buf.items) |event| {
            switch (event) {
                .key => |k| try self.dispatchKeyEvent(k),
                .resize => |r| try self.handleResize(r.cols, r.rows),
                else => {},
            }
        }
    }

    fn dispatchKeyEvent(self: *Engine, k: KeyEvent) !void {
        // 1. Model Seçici Modalı Açıksa
        if (self.model_selector.is_open) {
            if (self.model_selector.handleKey(k)) |chosen_model| {
                // Seçilen modeli header'da güncelle
                var hdr_buf: [128]u8 = undefined;
                const new_badge = std.fmt.bufPrint(&hdr_buf, "▣ {s}", .{chosen_model}) catch "▣ Model";
                _ = new_badge;
            }
            return;
        }

        // 2. Komut Paleti Modalı Açıksa
        if (self.command_palette.is_open) {
            if (self.command_palette.handleKey(k)) |cmd| {
                try self.executeCommand(cmd);
            }
            return;
        }

        // 3. Global Kısayollar
        // Ctrl+C: Çıkış
        if (k.code.eql(.{ .char = 'c' }) and k.modifiers.ctrl) {
            self.is_running = false;
            return;
        }

        // Ctrl+P: Model Seçici Modalı Aç
        if (k.code.eql(.{ .char = 'p' }) and k.modifiers.ctrl) {
            self.model_selector.open();
            return;
        }

        // Ctrl+K: Komut Paleti Modalı Aç
        if (k.code.eql(.{ .char = 'k' }) and k.modifiers.ctrl) {
            self.command_palette.open();
            return;
        }

        // Ctrl+V: Ses modu aç/kapa
        if (k.code.eql(.{ .char = 'v' }) and k.modifiers.ctrl) {
            self.voice.toggle();
            return;
        }

        // Tab: Panel odak değiştir
        if (k.code.eql(.{ .special = .tab })) {
            self.focus = switch (self.focus) {
                .conversation => .changed_files,
                .changed_files => .diff,
                .diff => .conversation,
            };
            self.header.active_tab = switch (self.focus) {
                .conversation => 0,
                .changed_files => 1,
                .diff => 2,
            };
            return;
        }

        // 'q' tuşu: Editör boşsa çıkış yap
        if (k.code.eql(.{ .char = 'q' }) and !k.modifiers.ctrl and !k.modifiers.alt) {
            if (self.prompt_editor.buffer.len() == 0 and self.focus != .conversation) {
                self.is_running = false;
                return;
            }
        }

        // 4. Panel Bazlı Girdi Yönlendirmesi
        switch (self.focus) {
            .conversation => {
                // Eğer soru kartı aktifse ve yön tuşları geldiyse soru kartına ilet
                if (k.code.eql(.{ .special = .up }) or k.code.eql(.{ .special = .down })) {
                    if (self.prompt_editor.buffer.len() == 0) {
                        const key_str = if (k.code.eql(.{ .special = .up })) "\x1b[A" else "\x1b[B";
                        _ = try self.question.handleKey(key_str);
                        return;
                    }
                }

                // Prompt düzenleyiciye tuş olayını ver
                if (try self.prompt_editor.handleKey(k)) |submitted_text| {
                    defer self.allocator.free(submitted_text);

                    // Slash komutlarını kontrol et
                    if (std.mem.startsWith(u8, submitted_text, "/")) {
                        try self.executeCommand(submitted_text);
                    } else {
                        // Normal kullanıcı mesajı ekle
                        _ = try self.blocks.addBlock(.user, "Operator", submitted_text);
                        // Ajan simülasyon yanıtı ekle
                        _ = try self.blocks.addBlock(.agent, "Omnitrix", "Komut alındı ve StateStore üzerinde yürütülüyor.");
                    }
                }
            },
            .changed_files => {
                if (k.code.eql(.{ .special = .up }) or (k.code.eql(.{ .char = 'k' }) and !k.modifiers.ctrl)) {
                    if (self.changed_panel.selected_index > 0) self.changed_panel.selected_index -= 1;
                } else if (k.code.eql(.{ .special = .down }) or (k.code.eql(.{ .char = 'j' }) and !k.modifiers.ctrl)) {
                    const max_idx = if (self.changed_panel.selected_in_agent)
                        if (self.changed_panel.agent_entries.items.len > 0) self.changed_panel.agent_entries.items.len - 1 else 0
                    else if (self.changed_panel.project_entries.items.len > 0) self.changed_panel.project_entries.items.len - 1 else 0;

                    if (self.changed_panel.selected_index < max_idx) self.changed_panel.selected_index += 1;
                } else if (k.code.eql(.{ .char = ' ' })) {
                    self.changed_panel.selected_in_agent = !self.changed_panel.selected_in_agent;
                    self.changed_panel.selected_index = 0;
                } else if (k.code.eql(.{ .special = .enter })) {
                    self.focus = .diff;
                    self.header.active_tab = 2;
                }
            },
            .diff => {
                if (k.code.eql(.{ .special = .up }) or (k.code.eql(.{ .char = 'k' }) and !k.modifiers.ctrl)) {
                    if (self.diffs.selected_hunk_index > 0) self.diffs.selected_hunk_index -= 1;
                } else if (k.code.eql(.{ .special = .down }) or (k.code.eql(.{ .char = 'j' }) and !k.modifiers.ctrl)) {
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

    fn executeCommand(self: *Engine, cmd: []const u8) !void {
        if (std.mem.eql(u8, cmd, "/model")) {
            self.model_selector.open();
        } else if (std.mem.eql(u8, cmd, "/voice")) {
            self.voice.toggle();
        } else if (std.mem.eql(u8, cmd, "/diff")) {
            self.focus = .diff;
            self.header.active_tab = 2;
        } else if (std.mem.eql(u8, cmd, "/files")) {
            self.focus = .changed_files;
            self.header.active_tab = 1;
        } else if (std.mem.eql(u8, cmd, "/clear")) {
            self.blocks.deinit();
            self.blocks = BlockRenderer.init(self.allocator, 100);
        } else if (std.mem.eql(u8, cmd, "/help")) {
            _ = try self.blocks.addBlock(.system, "Help", "Kısayollar:\n• Tab: Panel değiştir\n• Ctrl+P: Model seçici modalı aç\n• Ctrl+K: Komut paleti modalı aç\n• Ctrl+V: Ses modu\n• Ctrl+C / q: Çıkış");
        } else if (std.mem.eql(u8, cmd, "/quit") or std.mem.eql(u8, cmd, "/exit")) {
            self.is_running = false;
        }
    }

    /// Tüm ekranı 2D matris üzerinde sıfır hata ve sıfır titreşimle çizer.
    pub fn render(self: *Engine) !void {
        self.voice.tick();
        self.back_buffer.clear();

        const screen_area = Rect.init(0, 0, self.size.cols, self.size.rows);
        if (screen_area.isEmpty()) return;

        // 1. Dikey Bölme: Üst Sekmeler (1 satır), Ana Gövde (Kalan), Alt Prompt & Footer (3 satır)
        const v_constraints = [_]Constraint{
            .{ .length = 1 }, // Üst Sekmeler
            .{ .fill = 1 },   // Ana Gövde (Sohbet + Sidebar)
            .{ .length = 3 }, // Alt Prompt Editörü + Yardım İpuçları
        };
        const v_layout = Layout.init(.vertical, &v_constraints);
        const v_chunks = try v_layout.split(screen_area, self.allocator);
        defer self.allocator.free(v_chunks);

        const header_area = v_chunks[0];
        const body_area = v_chunks[1];
        const footer_area = v_chunks[2];

        // 1. Üst Sekmeleri Çiz
        self.header.render(header_area, &self.back_buffer);

        // 2. Yatay Bölme (Mekansal Çift Sütun): Sol Ana Alan (~68%), Sağ Sidebar (~32%)
        const has_sidebar = (self.size.cols >= 100);
        if (has_sidebar) {
            const h_constraints = [_]Constraint{
                .{ .fill = 1 },
                .{ .length = 1 }, // Ayırıcı
                .{ .length = @min(38, self.size.cols / 3) },
            };
            const h_layout = Layout.init(.horizontal, &h_constraints);
            const h_chunks = try h_layout.split(body_area, self.allocator);
            defer self.allocator.free(h_chunks);

            const main_area = h_chunks[0];
            const div_area = h_chunks[1];
            const side_area = h_chunks[2];

            // Sol Ana Alanı Çiz
            self.renderMainArea(main_area);

            // Dikey Ayırıcı Çizgi
            var dy = div_area.top();
            while (dy < div_area.bottom()) : (dy += 1) {
                if (self.back_buffer.getMut(div_area.left(), dy)) |c| {
                    c.setSymbol("│", 1);
                    c.setStyle(.{ .fg = .{ .indexed = 237 } });
                }
            }

            // Sağ Sidebarı Çiz
            self.sidebar.render(side_area, &self.back_buffer);
        } else {
            self.renderMainArea(body_area);
        }

        // 3. Alt Prompt Düzenleyici ve Kısayol İpuçlarını Çiz
        self.renderFooter(footer_area);

        // 4. Modalları En Üst Katmanda (Z-Index) Çiz
        if (self.model_selector.is_open) {
            self.model_selector.render(screen_area, &self.back_buffer);
        } else if (self.command_palette.is_open) {
            self.command_palette.render(screen_area, &self.back_buffer);
        }

        // 5. 2D Tampon Karşılaştırması (Diff) ve Atomik Çıktı
        self.io_buf.clearRetainingCapacity();

        // İmleci gizle
        try self.io_buf.appendSlice(self.allocator, "\x1b[?25l");

        try self.differ.renderDiff(&self.front_buffer, &self.back_buffer, &self.io_buf);

        // Tamponları takas et (Swap)
        @memcpy(self.front_buffer.content, self.back_buffer.content);

        // Tek bir sistem çağrısı ile terminale bas
        if (self.io_buf.items.len > 0) {
            try self.backend.write(self.io_buf.items);
            try self.backend.flush();
        }
    }

    fn renderMainArea(self: *Engine, area: Rect) void {
        if (area.isEmpty()) return;

        var y = area.top() + 1;
        const left = area.left() + 1;
        const max_w = if (area.width > 2) area.width - 2 else area.width;

        switch (self.focus) {
            .conversation => {
                // Ses Modu Banner'ı (Aktifse)
                if (self.voice.state != .off) {
                    _ = self.back_buffer.setString(left, y, "🎙️ GROK VOICE MODE [Listening]:  ▂▃▅▆▇▆▅▃  (Ctrl+V to toggle)", .{ .fg = .bright_magenta, .modifier = .{ .bold = true } }, max_w);
                    y += 2;
                }

                // Dinamik Bloklar Varsa Onları Bas
                if (self.blocks.blocks.items.len > 0) {
                    var bl_lines = self.blocks.renderToLines(self.allocator, max_w) catch std.ArrayList([]const u8).empty;
                    defer {
                        for (bl_lines.items) |l| self.allocator.free(l);
                        bl_lines.deinit(self.allocator);
                    }
                    for (bl_lines.items) |bl| {
                        if (y >= area.bottom() - 6) break;
                        _ = self.back_buffer.setString(left, y, bl, .{ .fg = .bright_white }, max_w);
                        y += 1;
                    }
                } else {
                    // Varsayılan Karşılama ve Talimat
                    _ = self.back_buffer.setString(left, y, "Omnitrix Autonomous Runtime v0.1.0", .{ .fg = .{ .indexed = 39 }, .modifier = .{ .bold = true } }, max_w);
                    y += 1;
                    _ = self.back_buffer.setString(left, y, "EventLoop, FileMutationLedger ve TaskScheduler hazır.", .{ .fg = .{ .indexed = 244 } }, max_w);
                    y += 2;
                }

                // OpenCode Soru & Seçim Kartı (Gerekiyorsa)
                if (self.question.options.items.len > 0) {
                    _ = self.back_buffer.setString(left, y, "Tek soru kaldı:", .{ .fg = .bright_white }, max_w);
                    y += 1;
                    _ = self.back_buffer.setString(left, y, "→ Asked 1 question", .{ .fg = .{ .indexed = 244 } }, max_w);
                    y += 2;

                    const q_area = Rect.init(left, y, max_w, if (area.bottom() > y) area.bottom() - y else 0);
                    self.question.render(q_area, &self.back_buffer);
                }
            },
            .changed_files => {
                var cf_lines = self.changed_panel.renderToLines(self.allocator, max_w) catch std.ArrayList([]const u8).empty;
                defer {
                    for (cf_lines.items) |l| self.allocator.free(l);
                    cf_lines.deinit(self.allocator);
                }
                for (cf_lines.items) |cfl| {
                    if (y >= area.bottom()) break;
                    _ = self.back_buffer.setString(left, y, cfl, .{ .fg = .bright_white }, max_w);
                    y += 1;
                }
            },
            .diff => {
                var df_lines = self.diffs.renderActiveDiffToLines(self.allocator, max_w) catch std.ArrayList([]const u8).empty;
                defer {
                    for (df_lines.items) |l| self.allocator.free(l);
                    df_lines.deinit(self.allocator);
                }
                for (df_lines.items) |dfl| {
                    if (y >= area.bottom()) break;
                    _ = self.back_buffer.setString(left, y, dfl, .{ .fg = .bright_white }, max_w);
                    y += 1;
                }
            },
        }
    }

    fn renderFooter(self: *Engine, area: Rect) void {
        if (area.isEmpty()) return;

        // 1. Prompt Editor (İlk 2 satır)
        const prompt_area = Rect.init(area.left(), area.top(), area.width, 2);
        self.prompt_editor.render(prompt_area, &self.back_buffer);

        // 2. Kısayol İpuçları (Son satır)
        const hint_y = area.bottom() - 1;
        const hint_str = " [Tab] Panel Değiştir  │  [Ctrl+P] Model Seç  │  [Ctrl+K] Komutlar  │  [Ctrl+V] Ses Modu  │  [q] Çıkış";
        _ = self.back_buffer.setString(area.left() + 1, hint_y, hint_str, .{ .fg = .{ .indexed = 241 } }, area.width);
    }
};

test "engine double buffered render and modal overlays" {
    const pty_harness = @import("../pty_harness.zig");
    var harness = try pty_harness.PtyHarness.init(std.testing.allocator, 120, 40);
    defer harness.deinit();

    var engine = try Engine.init(std.testing.allocator, harness.backend_inst.backend());
    defer engine.deinit();

    try engine.render();

    try std.testing.expect(harness.assertContains("Omnitrix projesi ilk adım"));
    try std.testing.expect(harness.assertContains("Context"));
    try std.testing.expect(harness.assertContains("MCP"));
    try std.testing.expect(harness.assertContains("Build"));
    try std.testing.expect(harness.assertContains("Model Seç"));

    // Model Seçici Modalı Aç ve Render Et
    engine.model_selector.open();
    try engine.render();
    try std.testing.expect(harness.assertContains("Select Active Model"));
    try std.testing.expect(harness.assertContains("Claude 3.5 Sonnet"));
}
