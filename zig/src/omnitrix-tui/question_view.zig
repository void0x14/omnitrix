//! omnitrix-tui: OpenCode / Grok Question View (crates/codegen/xai-grok-pager/src/views/question_view.rs Port)
//!
//! Özellikler:
//! - QuestionViewState: Çoklu soru yönetimi, tekli/çoklu seçim (Single/Multi), metin girişi (InputMode).
//! - QuestionOption: Başlık, açıklama (word-wrapped), önizleme metni.
//! - Numaralandırılmış liste (1..N) ve Serbest Cevap ("Type your own answer").
//! - Navigasyon: j/k veya Ok Tuşları, Enter (seçim/onay), Tab (mod değişimi), Esc (çıkış/iptal).
//! - Render: Sol dikey mavi/mor çizgi ('│'), aktif seçenek parlak cyan/mavi, girintili gri açıklamalar.

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");
const Style = term.Style;
const Color = term.Color;

pub const QuestionOption = struct {
    title: []const u8,
    description: ?[]const u8 = null,
    preview: ?[]const u8 = null,
};

pub const QuestionSelection = union(enum) {
    single: ?usize,
    multi: std.AutoHashMap(usize, void),
};

pub const QuestionFocus = enum {
    navigation,
    input_mode,
};

pub const QuestionViewState = struct {
    allocator: std.mem.Allocator,
    mode_name: []const u8 = "Build",
    model_name: []const u8 = "MiMo-V2.5-Pro",
    question_text: []const u8,
    options: std.ArrayList(QuestionOption),
    selection: QuestionSelection,
    focus: QuestionFocus = .navigation,
    cursor_idx: usize = 0,
    custom_input: std.ArrayList(u8),
    is_active: bool = true,
    is_multi_select: bool = false,

    pub fn init(allocator: std.mem.Allocator, question: []const u8, is_multi: bool) QuestionViewState {
        return .{
            .allocator = allocator,
            .mode_name = "Build",
            .model_name = "MiMo-V2.5-Pro",
            .question_text = question,
            .options = std.ArrayList(QuestionOption).empty,
            .selection = if (is_multi) QuestionSelection{ .multi = std.AutoHashMap(usize, void).init(allocator) } else QuestionSelection{ .single = 0 },
            .focus = .navigation,
            .cursor_idx = 0,
            .custom_input = std.ArrayList(u8).empty,
            .is_active = true,
            .is_multi_select = is_multi,
        };
    }

    pub fn deinit(self: *QuestionViewState) void {
        self.options.deinit(self.allocator);
        switch (self.selection) {
            .multi => |*map| map.deinit(),
            .single => {},
        }
        self.custom_input.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addOption(self: *QuestionViewState, title: []const u8, description: ?[]const u8) !void {
        try self.options.append(self.allocator, .{
            .title = title,
            .description = description,
            .preview = null,
        });
    }

    pub fn handleKey(self: *QuestionViewState, key: []const u8) !bool {
        if (!self.is_active) return false;

        const total_rows = self.options.items.len + 1; // +1 for "Type your own answer"

        if (self.focus == .navigation) {
            // Yukarı (Up / k)
            if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
                if (self.cursor_idx > 0) {
                    self.cursor_idx -= 1;
                    if (!self.is_multi_select and self.cursor_idx < self.options.items.len) {
                        self.selection = .{ .single = self.cursor_idx };
                    }
                    return true;
                }
            }
            // Aşağı (Down / j)
            else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
                if (self.cursor_idx + 1 < total_rows) {
                    self.cursor_idx += 1;
                    if (!self.is_multi_select and self.cursor_idx < self.options.items.len) {
                        self.selection = .{ .single = self.cursor_idx };
                    }
                    return true;
                }
            }
            // Enter
            else if (key.len == 1 and (key[0] == '\r' or key[0] == '\n')) {
                if (self.cursor_idx == self.options.items.len) {
                    // Serbest metin giriş moduna geç
                    self.focus = .input_mode;
                    return true;
                } else if (self.is_multi_select) {
                    switch (self.selection) {
                        .multi => |*map| {
                            if (map.contains(self.cursor_idx)) {
                                _ = map.remove(self.cursor_idx);
                            } else {
                                try map.put(self.cursor_idx, {});
                            }
                        },
                        .single => {},
                    }
                    return true;
                }
            }
        } else if (self.focus == .input_mode) {
            if (key.len == 1 and key[0] == 27) { // Esc
                self.focus = .navigation;
                return true;
            } else if (key.len == 1 and (key[0] == 127 or key[0] == 8)) { // Backspace
                if (self.custom_input.items.len > 0) {
                    _ = self.custom_input.pop();
                    return true;
                }
            } else if (key.len == 1 and key[0] >= 32 and key[0] <= 126) {
                try self.custom_input.append(self.allocator, key[0]);
                return true;
            }
        }

        return false;
    }

    pub fn renderToLines(self: *QuestionViewState, allocator: std.mem.Allocator, width: usize) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        var l_buf = std.ArrayList(u8).empty;
        defer l_buf.deinit(allocator);

        // 1. Mod / Model rozeti: ▣ Build · MiMo-V2.5-Pro
        try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 111 } }); // Mavi kare
        try l_buf.appendSlice(allocator, "▣ ");
        try term.appendStyle(&l_buf, allocator, .{ .fg = Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, self.mode_name);
        try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, " · ");
        try l_buf.appendSlice(allocator, self.model_name);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 2. Sol Çizgili Kart Başlığı (│ Soru)
        const bar_color = Color{ .ansi = 111 }; // Açık mor/mavi sol çizgi
        try term.appendStyle(&l_buf, allocator, .{ .fg = bar_color });
        try l_buf.appendSlice(allocator, "│ ");
        try term.appendStyle(&l_buf, allocator, .{ .fg = Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, self.question_text);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try term.appendStyle(&l_buf, allocator, .{ .fg = bar_color });
        try l_buf.appendSlice(allocator, "│");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        // 3. Seçenekler Listesi (1..N)
        for (self.options.items, 0..) |opt, idx| {
            const is_current = (self.cursor_idx == idx);

            try term.appendStyle(&l_buf, allocator, .{ .fg = bar_color });
            try l_buf.appendSlice(allocator, "│ ");

            var num_buf: [16]u8 = undefined;
            const num_str = std.fmt.bufPrint(&num_buf, "{d}. ", .{idx + 1}) catch "1. ";

            if (is_current) {
                try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 117 }, .bold = true }); // Parlak cyan/mavi
                try l_buf.appendSlice(allocator, num_str);
                try l_buf.appendSlice(allocator, opt.title);
            } else {
                try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 246 } });
                try l_buf.appendSlice(allocator, num_str);
                try term.appendStyle(&l_buf, allocator, .{ .fg = Color.bright_white });
                try l_buf.appendSlice(allocator, opt.title);
            }
            try l_buf.appendSlice(allocator, theme.ANSI.reset);
            try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
            l_buf.clearRetainingCapacity();

            // Girintili Açıklama Metni
            if (opt.description) |desc| {
                try term.appendStyle(&l_buf, allocator, .{ .fg = bar_color });
                try l_buf.appendSlice(allocator, "│    ");
                try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 244 } });
                try l_buf.appendSlice(allocator, desc);
                try l_buf.appendSlice(allocator, theme.ANSI.reset);

                const max_desc_w = if (width > 8) width - 8 else width;
                const trunc = try unicode.truncateToWidth(allocator, l_buf.items, max_desc_w, "...");
                defer allocator.free(trunc);
                try lines.append(allocator, try allocator.dupe(u8, trunc));
                l_buf.clearRetainingCapacity();
            }
        }

        // 4. "Type your own answer" Seçeneği
        const is_custom_selected = (self.cursor_idx == self.options.items.len);
        var custom_num_buf: [16]u8 = undefined;
        const custom_num_str = std.fmt.bufPrint(&custom_num_buf, "{d}. ", .{self.options.items.len + 1}) catch "4. ";

        try term.appendStyle(&l_buf, allocator, .{ .fg = bar_color });
        try l_buf.appendSlice(allocator, "│ ");

        if (is_custom_selected) {
            try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 117 }, .bold = true });
            try l_buf.appendSlice(allocator, custom_num_str);
            try l_buf.appendSlice(allocator, "Type your own answer");
            if (self.focus == .input_mode) {
                try l_buf.appendSlice(allocator, ": ");
                try l_buf.appendSlice(allocator, self.custom_input.items);
                try l_buf.appendSlice(allocator, "▋");
            }
        } else {
            try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 246 } });
            try l_buf.appendSlice(allocator, custom_num_str);
            try term.appendStyle(&l_buf, allocator, .{ .fg = Color.bright_white });
            try l_buf.appendSlice(allocator, "Type your own answer");
        }
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        // 5. Alt Kısayollar Satırı
        try term.appendStyle(&l_buf, allocator, .{ .fg = bar_color });
        try l_buf.appendSlice(allocator, "│");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try term.appendStyle(&l_buf, allocator, .{ .fg = Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, "  ↑↓ select   enter submit   esc dismiss");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));

        return lines;
    }
};

test "question view state navigasyon ve render" {
    var qv = QuestionViewState.init(std.testing.allocator, "Crush mekanizmalari entegrasyonu?", false);
    defer qv.deinit();

    try qv.addOption("Crush'dan al, ekle", "Crush'daki fold + iptal mekanizmalarini Omnitrix'e ekler.");
    try qv.addOption("Kendi kodundakileri kullan", "Kendi kodundaki doom_loop sinyalini kullanir.");
    try qv.addOption("Ikisini birlestir", "En iyi kisimlari birlestirir.");

    try std.testing.expect(qv.cursor_idx == 0);

    // Aşağı yön tuşu
    _ = try qv.handleKey("\x1b[B");
    try std.testing.expect(qv.cursor_idx == 1);

    var lines = try qv.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    try std.testing.expect(lines.items.len >= 8);
}
