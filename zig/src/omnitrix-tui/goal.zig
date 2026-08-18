//! omnitrix-tui: Codex & Claude Code Stili Hedef (Goal) ve İlerleme Takip Motoru
//!
//! Özellikler:
//! - Çok adımlı görev hiyerarşisi (completed [✓], active [▶], pending [○], failed [✗]).
//! - Canlı ASCII/Unicode ilerleme çubuğu ([████████░░░░] %66).
//! - OpenCode / Codex tarzı yuvarlak kenarlıklı kart çizimi.

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");

pub const StepStatus = enum {
    pending,
    active,
    completed,
    failed,
};

pub const GoalStep = struct {
    title: []const u8,
    status: StepStatus = .pending,
    duration_ms: ?u64 = null,

    pub fn deinit(self: *GoalStep, allocator: std.mem.Allocator) void {
        allocator.free(self.title);
        self.* = undefined;
    }
};

pub const GoalTracker = struct {
    allocator: std.mem.Allocator,
    title: []const u8,
    steps: std.ArrayList(GoalStep),
    is_active: bool = true,
    is_collapsed: bool = false,

    pub fn init(allocator: std.mem.Allocator, title: []const u8) !GoalTracker {
        return .{
            .allocator = allocator,
            .title = try allocator.dupe(u8, title),
            .steps = std.ArrayList(GoalStep).empty,
            .is_active = true,
            .is_collapsed = false,
        };
    }

    pub fn deinit(self: *GoalTracker) void {
        self.allocator.free(self.title);
        for (self.steps.items) |*s| s.deinit(self.allocator);
        self.steps.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addStep(self: *GoalTracker, step_title: []const u8, status: StepStatus) !void {
        const title_copy = try self.allocator.dupe(u8, step_title);
        errdefer self.allocator.free(title_copy);
        try self.steps.append(self.allocator, .{
            .title = title_copy,
            .status = status,
        });
    }

    pub fn completeStep(self: *GoalTracker, index: usize) void {
        if (index < self.steps.items.len) {
            self.steps.items[index].status = .completed;
            // Sıradaki pending adımı active yap
            if (index + 1 < self.steps.items.len and self.steps.items[index + 1].status == .pending) {
                self.steps.items[index + 1].status = .active;
            }
        }
    }

    pub fn progressPercent(self: *const GoalTracker) u8 {
        if (self.steps.items.len == 0) return 0;
        var completed: usize = 0;
        for (self.steps.items) |s| {
            if (s.status == .completed) completed += 1;
        }
        return @intCast((completed * 100) / self.steps.items.len);
    }

    /// Hedef kartını metin satırları olarak render eder.
    pub fn renderToLines(self: *const GoalTracker, allocator: std.mem.Allocator, width: usize) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        if (!self.is_active or width < 20) return lines;
        const inner_width = if (width > 4) width - 4 else 16;

        // 1. Üst Kenarlık: ╭─ 🎯 Goal: [Title] ────────╮
        var header_buf = std.ArrayList(u8).empty;
        defer header_buf.deinit(allocator);

        try term.appendStyle(&header_buf, allocator, .{ .fg = theme.Theme.border, .bold = false });
        try header_buf.appendSlice(allocator, theme.Box.top_left);
        try header_buf.appendSlice(allocator, theme.Box.horizontal);
        try term.appendStyle(&header_buf, allocator, theme.Theme.header_goal);
        try header_buf.appendSlice(allocator, " 🎯 Goal: ");
        try header_buf.appendSlice(allocator, self.title);
        try header_buf.appendSlice(allocator, " ");
        try term.appendStyle(&header_buf, allocator, .{ .fg = theme.Theme.border, .bold = false });

        const header_rendered_width = unicode.strWidth(self.title) + 13;
        var pad: usize = if (width > header_rendered_width + 1) width - header_rendered_width - 1 else 1;
        while (pad > 0) : (pad -= 1) {
            try header_buf.appendSlice(allocator, theme.Box.horizontal);
        }
        try header_buf.appendSlice(allocator, theme.Box.top_right);
        try header_buf.appendSlice(allocator, theme.ANSI.reset);

        try lines.append(allocator, try header_buf.toOwnedSlice(allocator));

        if (!self.is_collapsed) {
            // 2. Adımlar Listesi
            for (self.steps.items, 0..) |s, idx| {
                _ = idx;
                var step_buf = std.ArrayList(u8).empty;
                defer step_buf.deinit(allocator);

                try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.border });
                try step_buf.appendSlice(allocator, theme.Box.vertical);
                try step_buf.appendSlice(allocator, "  ");

                switch (s.status) {
                    .completed => {
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_green, .bold = true });
                        try step_buf.appendSlice(allocator, "[✓] ");
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_dim, .dim = true });
                    },
                    .active => {
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_cyan, .bold = true });
                        try step_buf.appendSlice(allocator, "[▶] ");
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_main, .bold = true });
                    },
                    .pending => {
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_dim });
                        try step_buf.appendSlice(allocator, "[○] ");
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_dim });
                    },
                    .failed => {
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_red, .bold = true });
                        try step_buf.appendSlice(allocator, "[✗] ");
                        try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.text_red });
                    },
                }

                const truncated_step = try unicode.truncateToWidth(allocator, s.title, inner_width - 8, "...");
                defer allocator.free(truncated_step);
                try step_buf.appendSlice(allocator, truncated_step);
                try term.appendStyle(&step_buf, allocator, .{ .fg = theme.Theme.border });

                const current_w = unicode.strWidth(truncated_step) + 8;
                var r_pad = if (width > current_w + 2) width - current_w - 2 else 1;
                while (r_pad > 0) : (r_pad -= 1) {
                    try step_buf.appendSlice(allocator, " ");
                }
                try step_buf.appendSlice(allocator, theme.Box.vertical);
                try step_buf.appendSlice(allocator, theme.ANSI.reset);

                try lines.append(allocator, try step_buf.toOwnedSlice(allocator));
            }

            // 3. İlerleme Çubuğu Satırı
            var prog_buf = std.ArrayList(u8).empty;
            defer prog_buf.deinit(allocator);

            const pct = self.progressPercent();
            const bar_len: usize = if (inner_width > 30) 20 else 10;
            const filled_len: usize = (bar_len * pct) / 100;

            try term.appendStyle(&prog_buf, allocator, .{ .fg = theme.Theme.border });
            try prog_buf.appendSlice(allocator, theme.Box.vertical);
            try prog_buf.appendSlice(allocator, "  Progress: [");

            try term.appendStyle(&prog_buf, allocator, .{ .fg = theme.Theme.text_green, .bold = true });
            var f: usize = 0;
            while (f < filled_len) : (f += 1) {
                try prog_buf.appendSlice(allocator, theme.Box.prog_full);
            }
            try term.appendStyle(&prog_buf, allocator, .{ .fg = theme.Theme.text_dim });
            var e: usize = filled_len;
            while (e < bar_len) : (e += 1) {
                try prog_buf.appendSlice(allocator, theme.Box.prog_empty);
            }
            try term.appendStyle(&prog_buf, allocator, .{ .fg = theme.Theme.border });
            try prog_buf.appendSlice(allocator, "] ");

            var pct_str: [32]u8 = undefined;
            const pct_line = try std.fmt.bufPrint(&pct_str, "{d}% ({d}/{d} steps)", .{
                pct,
                (self.steps.items.len * pct) / 100,
                self.steps.items.len,
            });
            try term.appendStyle(&prog_buf, allocator, .{ .fg = theme.Theme.text_main, .bold = true });
            try prog_buf.appendSlice(allocator, pct_line);

            const total_prog_w = 14 + bar_len + pct_line.len;
            var prog_pad = if (width > total_prog_w + 2) width - total_prog_w - 2 else 1;
            try term.appendStyle(&prog_buf, allocator, .{ .fg = theme.Theme.border });
            while (prog_pad > 0) : (prog_pad -= 1) {
                try prog_buf.appendSlice(allocator, " ");
            }
            try prog_buf.appendSlice(allocator, theme.Box.vertical);
            try prog_buf.appendSlice(allocator, theme.ANSI.reset);

            try lines.append(allocator, try prog_buf.toOwnedSlice(allocator));
        }

        // 4. Alt Kenarlık: ╰──────────────────────────────────╯
        var bot_buf = std.ArrayList(u8).empty;
        defer bot_buf.deinit(allocator);

        try term.appendStyle(&bot_buf, allocator, .{ .fg = theme.Theme.border });
        try bot_buf.appendSlice(allocator, theme.Box.bottom_left);
        var b_pad: usize = if (width > 2) width - 2 else 1;
        while (b_pad > 0) : (b_pad -= 1) {
            try bot_buf.appendSlice(allocator, theme.Box.horizontal);
        }
        try bot_buf.appendSlice(allocator, theme.Box.bottom_right);
        try bot_buf.appendSlice(allocator, theme.ANSI.reset);

        try lines.append(allocator, try bot_buf.toOwnedSlice(allocator));

        return lines;
    }
};

test "goal tracker adimlari, ilerleme ve render" {
    var goal = try GoalTracker.init(std.testing.allocator, "Build Omnitrix Zig Runtime");
    defer goal.deinit();

    try goal.addStep("Step 1: omnitrix-io", .completed);
    try goal.addStep("Step 2: omnitrix-task", .completed);
    try goal.addStep("Step 3: OpenCode TUI", .active);
    try goal.addStep("Step 4: Grok Voice Mode", .pending);

    try std.testing.expectEqual(@as(u8, 50), goal.progressPercent());

    var lines = try goal.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    try std.testing.expect(lines.items.len >= 6);
}
