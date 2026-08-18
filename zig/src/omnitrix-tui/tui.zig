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
const engine_mod = @import("runtime/engine.zig");

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
pub const Engine = engine_mod.Engine;

pub const FocusPanel = enum {
    conversation,
    changed_files,
    diff,
};

pub const Tui = struct {
    allocator: std.mem.Allocator,
    backend: TerminalBackend,
    engine: *Engine,
    blocks: *BlockRenderer,
    diffs: *DiffRenderer,
    changed_panel: *ChangedFilesPanel,
    goal: *GoalTracker,
    voice: *VoiceMode,

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
        _ = max_blocks;
        const size = backend.getSize() catch TerminalSize{ .cols = 100, .rows = 30 };

        const engine_ptr = try allocator.create(Engine);
        errdefer allocator.destroy(engine_ptr);

        engine_ptr.* = try Engine.init(allocator, backend);

        return .{
            .allocator = allocator,
            .backend = backend,
            .engine = engine_ptr,
            .blocks = &engine_ptr.blocks,
            .diffs = &engine_ptr.diffs,
            .changed_panel = &engine_ptr.changed_panel,
            .goal = &engine_ptr.goal,
            .voice = &engine_ptr.voice,
            .focus = .conversation,
            .size = size,
            .is_running = true,
            .branch_name = "masterplan",
            .session_id = "omni_01",
            .turn_id = 1,
        };
    }

    pub fn deinit(self: *Tui) void {
        self.engine.deinit();
        self.allocator.destroy(self.engine);
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

        // Ctrl+V: Ses modu aç/kapat
        if (key.len == 1 and (key[0] == 22 or key[0] == '\x16')) {
            self.voice.toggle();
            self.engine.voice.toggle();
            return;
        }

        // 'q': Çıkış
        if (key.len == 1 and key[0] == 'q') {
            self.is_running = false;
            self.engine.is_running = false;
            return;
        }

        // Tab: Odak panelini değiştir
        if (key.len == 1 and key[0] == '\t') {
            self.cycleFocus();
            self.engine.focus = switch (self.focus) {
                .conversation => .conversation,
                .changed_files => .changed_files,
                .diff => .diff,
            };
            self.engine.header.active_tab = switch (self.focus) {
                .conversation => 0,
                .changed_files => 1,
                .diff => 2,
            };
            return;
        }

        // 't': Tool bloklarını katla/aç
        if (key.len == 1 and key[0] == 't') {
            for (self.blocks.blocks.items) |*b| {
                if (b.kind == .tool) {
                    b.tool_collapsed = !b.tool_collapsed;
                }
            }
            return;
        }

        try self.engine.handleKey(key);
        self.is_running = self.engine.is_running;
    }

    /// Tüm TUI ekranını 2D Hücre Matrisi ve BufferDiff motoru ile çizer
    pub fn renderFrame(self: *Tui) !void {
        self.engine.focus = switch (self.focus) {
            .conversation => .conversation,
            .changed_files => .changed_files,
            .diff => .diff,
        };

        try self.engine.render();
        self.is_running = self.engine.is_running;
    }
};

test "opencode 1.18.18 tui layout, sidebar, question box ve voice render" {
    const pty_harness_mod = @import("pty_harness.zig");
    var harness = try pty_harness_mod.PtyHarness.init(std.testing.allocator, 120, 40);
    defer harness.deinit();

    _ = try harness.tui.blocks.addBlock(.user, "Operator", "Sorun şu: Feature matrix'deki mekanizmaları al.");
    harness.tui.voice.state = .listening;

    try harness.tui.renderFrame();

    try std.testing.expect(harness.assertContains("Omnitrix projesi ilk adım"));
    try std.testing.expect(harness.assertContains("Context"));
    try std.testing.expect(harness.assertContains("MCP"));
    try std.testing.expect(harness.assertContains("Build"));
    try std.testing.expect(harness.assertContains("MiMo-V2.5-Pro"));
    try std.testing.expect(harness.assertContains("Crush"));
    try std.testing.expect(harness.assertContains("OpenCode 1.18.18"));
    try std.testing.expect(harness.assertContains("GROK VOICE MODE"));
}
