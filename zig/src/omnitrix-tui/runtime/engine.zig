//! omnitrix-tui: Master 2D TUI Engine & Runtime (Çift Tamponlu, Hücre Bazlı Motor)
//!
//! Özellikler:
//! - FrontBuffer ve BackBuffer çift tampon mimarisi (Double Buffering)
//! - Constraint Layout tabanlı ekran bölme (Üst Sekmeler, Sol Ana Akış, Sağ Sidebar)
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

        return .{
            .allocator = allocator,
            .backend = backend,
            .size = size,
            .front_buffer = front,
            .back_buffer = back,
            .differ = BufferDiff.init(allocator),
            .header = HeaderView.init(),
            .sidebar = SidebarView.init(allocator),
            .question = QuestionView.init(allocator, "Crush'da olan ama senin kodunda olmayan 3 mekanizma (prompt birleştirme, kesin iptal, döngü tespiti) için hangisini yapalım?"),
            .voice = VoiceMode.init(allocator),
            .goal = default_goal,
            .blocks = BlockRenderer.init(allocator, 100),
            .diffs = diffs_inst,
            .changed_panel = changed_inst,
            .focus = .conversation,
            .is_running = true,
            .io_buf = std.ArrayList(u8).empty,
        };
    }

    pub fn deinit(self: *Engine) void {
        self.front_buffer.deinit();
        self.back_buffer.deinit();
        self.sidebar.deinit();
        self.question.deinit();
        self.voice.deinit();
        self.goal.deinit();
        self.blocks.deinit();
        self.diffs.deinit();
        self.changed_panel.deinit();
        self.io_buf.deinit(self.allocator);
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

    pub fn handleKey(self: *Engine, key: []const u8) !void {
        if (key.len == 1 and key[0] == 'q') {
            self.is_running = false;
            return;
        }

        // Tab: Panel odak değiştir
        if (key.len == 1 and key[0] == '\t') {
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

        // Ctrl+V: Ses modu aç/kapa
        if (key.len == 1 and key[0] == '\x16') {
            self.voice.toggle();
            return;
        }

        _ = try self.question.handleKey(key);
    }

    /// Tüm ekranı 2D matris üzerinde sıfır hata ve sıfır titreşimle çizer.
    pub fn render(self: *Engine) !void {
        self.voice.tick();
        self.back_buffer.clear();

        const screen_area = Rect.init(0, 0, self.size.cols, self.size.rows);
        if (screen_area.isEmpty()) return;

        // 1. Dikey Bölme: Üst Sekmeler (1 satır), Ana Gövde (Kalan)
        const v_constraints = [_]Constraint{
            .{ .length = 1 },
            .{ .fill = 1 },
        };
        const v_layout = Layout.init(.vertical, &v_constraints);
        const v_chunks = try v_layout.split(screen_area, self.allocator);
        defer self.allocator.free(v_chunks);

        const header_area = v_chunks[0];
        const body_area = v_chunks[1];

        // Üst Sekmeleri Çiz
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

        // 3. 2D Tampon Karşılaştırması (Diff) ve Atomik Çıktı
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
                    // Varsayılan OpenCode Diyaloğu
                    _ = self.back_buffer.setString(left, y, "Sorun şu: Feature matrix'deki 3 mekanizmayı al.", .{ .fg = .{ .indexed = 221 }, .modifier = .{ .bold = true } }, max_w);
                    y += 1;
                    _ = self.back_buffer.setString(left, y, "Yani seçenek:", .{ .fg = .bright_white }, max_w);
                    y += 1;
                    _ = self.back_buffer.setString(left, y, "- Seçenek A: Crush'daki mekanizmaları alıp Omnitrix'e ekleyeceğiz (fold + iptal + döngü tespiti)", .{ .fg = .{ .indexed = 221 } }, max_w);
                    y += 1;
                    _ = self.back_buffer.setString(left, y, "- Seçenek B: Senin koddaki mekanizmaları kullanacağız, crush'dan bir şey eklemeyeceğiz", .{ .fg = .{ .indexed = 221 } }, max_w);
                    y += 1;
                    _ = self.back_buffer.setString(left, y, "- Seçenek C: İkisinin de en iyi kısımlarını birleştireceğiz", .{ .fg = .{ .indexed = 221 } }, max_w);
                    y += 2;
                }

                _ = self.back_buffer.setString(left, y, "Tek soru kaldı:", .{ .fg = .bright_white }, max_w);
                y += 1;
                _ = self.back_buffer.setString(left, y, "→ Asked 1 question", .{ .fg = .{ .indexed = 244 } }, max_w);
                y += 2;

                // OpenCode Soru & Seçim Kartı
                const q_area = Rect.init(left, y, max_w, if (area.bottom() > y) area.bottom() - y else 0);
                self.question.render(q_area, &self.back_buffer);
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
};

test "engine double buffered render" {
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
    try std.testing.expect(harness.assertContains("MiMo-V2.5-Pro"));
    try std.testing.expect(harness.assertContains("Crush"));
    try std.testing.expect(harness.assertContains("OpenCode 1.18.18"));
}
