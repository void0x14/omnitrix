//! omnitrix-tui: OpenCode 1.18.18 Soru / Seçim Kartı ve Kompozitör
//!
//! Özellikler (Screenshot 1:1 Birebir):
//! - Mod/Model Etiketi: "▣ Build · MiMo-V2.5-Pro"
//! - Sol Dikey Çizgi (Accent Bar): '│' (Açık mor/mavi)
//! - Soru / Görev Metni
//! - Numaralandırılmış Seçenekler (1..N):
//!   - Seçili olan: Parlak mavi/cyan başlık, altında girintili açıklama
//!   - Diğerleri: Beyaz başlık, altında girintili gri açıklama
//!   - "Type your own answer" seçeneği
//! - Alt Kısayollar: "↑↓ select   enter submit   esc dismiss"

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");

pub const QuestionChoice = struct {
    title: []const u8,
    description: ?[]const u8 = null,
};

pub const QuestionBox = struct {
    allocator: std.mem.Allocator,
    mode_name: []const u8 = "Build",
    model_name: []const u8 = "MiMo-V2.5-Pro",
    question_text: []const u8,
    choices: std.ArrayList(QuestionChoice),
    selected_idx: usize = 0,
    is_active: bool = true,

    pub fn init(allocator: std.mem.Allocator, question: []const u8) QuestionBox {
        return .{
            .allocator = allocator,
            .mode_name = "Build",
            .model_name = "MiMo-V2.5-Pro",
            .question_text = question,
            .choices = std.ArrayList(QuestionChoice).empty,
            .selected_idx = 0,
            .is_active = true,
        };
    }

    pub fn deinit(self: *QuestionBox) void {
        self.choices.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addChoice(self: *QuestionBox, title: []const u8, description: ?[]const u8) !void {
        try self.choices.append(self.allocator, .{
            .title = title,
            .description = description,
        });
    }

    pub fn handleKey(self: *QuestionBox, key: []const u8) bool {
        if (!self.is_active) return false;

        // Yukarı yön tuşu (Arrow Up)
        if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
            if (self.selected_idx > 0) {
                self.selected_idx -= 1;
                return true;
            }
        }
        // Aşağı yön tuşu (Arrow Down)
        else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
            if (self.selected_idx + 1 < self.choices.items.len) {
                self.selected_idx += 1;
                return true;
            }
        }

        return false;
    }

    pub fn renderToLines(self: *QuestionBox, allocator: std.mem.Allocator, width: usize) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        var l_buf = std.ArrayList(u8).empty;
        defer l_buf.deinit(allocator);

        // 1. Mod ve Model Göstergesi: ▣ Build · MiMo-V2.5-Pro
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 111 } }); // Mavi kare
        try l_buf.appendSlice(allocator, "▣ ");
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, self.mode_name);
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, " · ");
        try l_buf.appendSlice(allocator, self.model_name);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 2. Sol Çizgili Soru Kartı
        // Başlık
        const bar_style = term.Style{ .fg = term.Color{ .ansi = 111 } }; // Açık mavi/mor sol çizgi

        try term.appendStyle(&l_buf, allocator, bar_style);
        try l_buf.appendSlice(allocator, "│ ");
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, self.question_text);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try term.appendStyle(&l_buf, allocator, bar_style);
        try l_buf.appendSlice(allocator, "│");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        // Seçenekler
        for (self.choices.items, 0..) |choice, idx| {
            const is_selected = (idx == self.selected_idx);

            try term.appendStyle(&l_buf, allocator, bar_style);
            try l_buf.appendSlice(allocator, "│ ");

            var num_buf: [16]u8 = undefined;
            const num_str = std.fmt.bufPrint(&num_buf, "{d}. ", .{idx + 1}) catch "1. ";

            if (is_selected) {
                try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 117 }, .bold = true }); // Parlak cyan/mavi
                try l_buf.appendSlice(allocator, num_str);
                try l_buf.appendSlice(allocator, choice.title);
            } else {
                try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 246 } });
                try l_buf.appendSlice(allocator, num_str);
                try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white });
                try l_buf.appendSlice(allocator, choice.title);
            }

            try l_buf.appendSlice(allocator, theme.ANSI.reset);
            try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
            l_buf.clearRetainingCapacity();

            // Açıklama varsa girintili ekle
            if (choice.description) |desc| {
                try term.appendStyle(&l_buf, allocator, bar_style);
                try l_buf.appendSlice(allocator, "│    ");
                try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 244 } });
                try l_buf.appendSlice(allocator, desc);
                try l_buf.appendSlice(allocator, theme.ANSI.reset);

                // Uzun satırları sar
                const max_text_w = if (width > 8) width - 8 else width;
                const trunc = try unicode.truncateToWidth(allocator, l_buf.items, max_text_w, "...");
                defer allocator.free(trunc);
                try lines.append(allocator, try allocator.dupe(u8, trunc));
                l_buf.clearRetainingCapacity();
            }
        }

        // 3. Alt Kısayollar
        try term.appendStyle(&l_buf, allocator, bar_style);
        try l_buf.appendSlice(allocator, "│");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, "  ↑↓ select   enter submit   esc dismiss");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));

        return lines;
    }
};

test "question box render lines" {
    var qb = QuestionBox.init(std.testing.allocator, "Test question?");
    defer qb.deinit();

    try qb.addChoice("Option 1", "Description 1");
    try qb.addChoice("Option 2", "Description 2");

    var lines = try qb.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    try std.testing.expect(lines.items.len > 4);
}
