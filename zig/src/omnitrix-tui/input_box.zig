//! omnitrix-tui: OpenCode Stili Etkileşimli Giriş Kutusu (Prompt Textarea & Autocomplete)
//!
//! Özellikler:
//! - Çok satırlı / tek satırlı zengin prompt giriş alanı.
//! - İmleç yönetimi, geçmiş (history up/down), tuş işleme.
//! - Slash komut menüsü (`/goal`, `/voice`, `/plan`, `/review`, `/diff`, `/files`, `/model`, `/help`).
//! - Alt bilgi çubuğu: Aktif Model, Mod (Build/Plan/Explore), Sesli Mod ve Token göstergesi.

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");
const Style = term.Style;
const Color = term.Color;

pub const InputBox = struct {
    allocator: std.mem.Allocator,
    text: std.ArrayList(u8),
    cursor_pos: usize = 0,
    history: std.ArrayList([]const u8),
    history_idx: ?usize = null,

    mode_name: []const u8 = "Build Mode",
    model_name: []const u8 = "Claude 3.5 Sonnet",
    voice_active: bool = false,
    token_count: usize = 2450,

    is_focused: bool = true,
    show_autocomplete: bool = false,
    autocomplete_idx: usize = 0,

    pub const slash_commands = [_][]const u8{
        "/goal    - Görev hedeflerini ve ilerlemeyi aç/kapat",
        "/voice   - Grok sesli modu (Voice Mode) aç/kapat",
        "/plan    - Salt-okunur plan moduna geç",
        "/review  - Değişiklikleri incele ve diff görüntüle",
        "/diff    - Tam ekran diff paneline odaklan",
        "/files   - Değişen dosyalar paneline odaklan",
        "/model   - Model ve sağlayıcı değiştir",
        "/clear   - Konuşma geçmişini temizle",
        "/help    - Komut yardımını göster",
    };

    pub fn init(allocator: std.mem.Allocator) InputBox {
        return .{
            .allocator = allocator,
            .text = std.ArrayList(u8).empty,
            .cursor_pos = 0,
            .history = std.ArrayList([]const u8).empty,
            .history_idx = null,
            .mode_name = "Build Mode",
            .model_name = "Claude 3.5 Sonnet",
            .voice_active = false,
            .token_count = 2450,
            .is_focused = true,
            .show_autocomplete = false,
            .autocomplete_idx = 0,
        };
    }

    pub fn deinit(self: *InputBox) void {
        self.text.deinit(self.allocator);
        for (self.history.items) |h| self.allocator.free(h);
        self.history.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn insertChar(self: *InputBox, char: u8) !void {
        try self.text.insert(self.allocator, self.cursor_pos, char);
        self.cursor_pos += 1;
        self.checkAutocomplete();
    }

    pub fn insertSlice(self: *InputBox, slice: []const u8) !void {
        for (slice) |c| {
            try self.insertChar(c);
        }
    }

    pub fn backspace(self: *InputBox) void {
        if (self.cursor_pos > 0 and self.text.items.len > 0) {
            _ = self.text.orderedRemove(self.cursor_pos - 1);
            self.cursor_pos -= 1;
            self.checkAutocomplete();
        }
    }

    pub fn clear(self: *InputBox) void {
        self.text.clearRetainingCapacity();
        self.cursor_pos = 0;
        self.show_autocomplete = false;
    }

    pub fn submit(self: *InputBox) !?[]const u8 {
        if (self.text.items.len == 0) return null;
        const msg = try self.allocator.dupe(u8, self.text.items);
        const hist_copy = try self.allocator.dupe(u8, self.text.items);
        try self.history.append(self.allocator, hist_copy);
        self.clear();
        return msg;
    }

    fn checkAutocomplete(self: *InputBox) void {
        if (self.text.items.len > 0 and self.text.items[0] == '/') {
            self.show_autocomplete = true;
        } else {
            self.show_autocomplete = false;
        }
    }

    pub fn handleKey(self: *InputBox, key: []const u8) !bool {
        if (key.len == 0) return false;

        // Enter
        if (key.len == 1 and (key[0] == '\r' or key[0] == '\n')) {
            if (self.show_autocomplete) {
                // Seçili komutu tamamla
                const cmd_line = slash_commands[self.autocomplete_idx];
                var space_idx: usize = 0;
                while (space_idx < cmd_line.len and cmd_line[space_idx] != ' ') : (space_idx += 1) {}
                const cmd_name = cmd_line[0..space_idx];

                self.clear();
                try self.insertSlice(cmd_name);
                self.show_autocomplete = false;
                return true;
            }
            return true; // Submit edilebilir
        }

        // Backspace
        if (key.len == 1 and (key[0] == 127 or key[0] == 8)) {
            self.backspace();
            return true;
        }

        // Yön tuşları
        if (key.len == 3 and key[0] == '\x1b' and key[1] == '[') {
            switch (key[2]) {
                'D' => { // Sol
                    if (self.cursor_pos > 0) self.cursor_pos -= 1;
                    return true;
                },
                'C' => { // Sağ
                    if (self.cursor_pos < self.text.items.len) self.cursor_pos += 1;
                    return true;
                },
                'A' => { // Yukarı (autocomplete veya history)
                    if (self.show_autocomplete) {
                        if (self.autocomplete_idx > 0) self.autocomplete_idx -= 1;
                    }
                    return true;
                },
                'B' => { // Aşağı (autocomplete veya history)
                    if (self.show_autocomplete) {
                        if (self.autocomplete_idx + 1 < slash_commands.len) self.autocomplete_idx += 1;
                    }
                    return true;
                },
                else => {},
            }
        }

        // Normal karakter yazma
        if (key.len == 1 and key[0] >= 32 and key[0] <= 126) {
            try self.insertChar(key[0]);
            return true;
        }

        return false;
    }

    /// Giriş kutusunu metin satırları olarak render eder.
    pub fn renderToLines(self: *const InputBox, allocator: std.mem.Allocator, width: usize) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        if (width < 30) return lines;
        const border_style = if (self.is_focused) Style{ .fg = theme.Theme.border_focused, .bold = true } else Style{ .fg = theme.Theme.border };

        // 0. Açılır Slash Komut Menüsü (Autocomplete Pop-up)
        if (self.show_autocomplete) {
            var ac_top = std.ArrayList(u8).empty;
            defer ac_top.deinit(allocator);
            try term.appendStyle(&ac_top, allocator, .{ .fg = theme.Theme.border_focused });
            try ac_top.appendSlice(allocator, "  ╭─ ⚡ Commands (Up/Down to select, Enter to choose) ─");
            var ac_pad: usize = if (width > 60) width - 60 else 1;
            while (ac_pad > 0) : (ac_pad -= 1) {
                try ac_top.appendSlice(allocator, theme.Box.horizontal);
            }
            try ac_top.appendSlice(allocator, theme.Box.top_right);
            try ac_top.appendSlice(allocator, theme.ANSI.reset);
            try lines.append(allocator, try ac_top.toOwnedSlice(allocator));

            for (slash_commands, 0..) |cmd, idx| {
                var ac_line = std.ArrayList(u8).empty;
                defer ac_line.deinit(allocator);

                const is_selected = (idx == self.autocomplete_idx);
                try term.appendStyle(&ac_line, allocator, .{ .fg = theme.Theme.border_focused });
                try ac_line.appendSlice(allocator, "  │ ");

                if (is_selected) {
                    try term.appendStyle(&ac_line, allocator, .{ .fg = theme.Theme.text_cyan, .bg = Color{ .ansi = 237 }, .bold = true });
                    try ac_line.appendSlice(allocator, " ▶ ");
                    try ac_line.appendSlice(allocator, cmd);
                } else {
                    try term.appendStyle(&ac_line, allocator, .{ .fg = theme.Theme.text_dim });
                    try ac_line.appendSlice(allocator, "   ");
                    try ac_line.appendSlice(allocator, cmd);
                }

                const cmd_w = unicode.strWidth(cmd) + 7;
                var ac_r_pad = if (width > cmd_w + 4) width - cmd_w - 4 else 1;
                while (ac_r_pad > 0) : (ac_r_pad -= 1) {
                    try ac_line.appendSlice(allocator, " ");
                }
                try term.appendStyle(&ac_line, allocator, .{ .fg = theme.Theme.border_focused });
                try ac_line.appendSlice(allocator, theme.Box.vertical);
                try ac_line.appendSlice(allocator, theme.ANSI.reset);

                try lines.append(allocator, try ac_line.toOwnedSlice(allocator));
            }

            var ac_bot = std.ArrayList(u8).empty;
            defer ac_bot.deinit(allocator);
            try term.appendStyle(&ac_bot, allocator, .{ .fg = theme.Theme.border_focused });
            try ac_bot.appendSlice(allocator, "  ╰");
            var ac_b_pad: usize = if (width > 6) width - 6 else 1;
            while (ac_b_pad > 0) : (ac_b_pad -= 1) {
                try ac_bot.appendSlice(allocator, theme.Box.horizontal);
            }
            try ac_bot.appendSlice(allocator, theme.Box.bottom_right);
            try ac_bot.appendSlice(allocator, theme.ANSI.reset);
            try lines.append(allocator, try ac_bot.toOwnedSlice(allocator));
        }

        // 1. Üst Kenarlık: ╭─ ❯ Prompt (Type / for commands, @ for files) ─╮
        var top_buf = std.ArrayList(u8).empty;
        defer top_buf.deinit(allocator);

        try term.appendStyle(&top_buf, allocator, border_style);
        try top_buf.appendSlice(allocator, theme.Box.top_left);
        try top_buf.appendSlice(allocator, theme.Box.horizontal);
        try term.appendStyle(&top_buf, allocator, .{ .fg = theme.Theme.text_cyan, .bold = true });
        try top_buf.appendSlice(allocator, " ❯ Prompt ");
        try term.appendStyle(&top_buf, allocator, .{ .fg = theme.Theme.text_dim, .dim = true });
        try top_buf.appendSlice(allocator, "(Type / for commands, @ for files) ");
        try term.appendStyle(&top_buf, allocator, border_style);

        const top_title_w = unicode.strWidth(" ❯ Prompt (Type / for commands, @ for files) ") + 2;
        var t_pad: usize = if (width > top_title_w + 1) width - top_title_w - 1 else 1;
        while (t_pad > 0) : (t_pad -= 1) {
            try top_buf.appendSlice(allocator, theme.Box.horizontal);
        }
        try top_buf.appendSlice(allocator, theme.Box.top_right);
        try top_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try top_buf.toOwnedSlice(allocator));

        // 2. Giriş Satırı: │ > user text|                        │
        var input_line = std.ArrayList(u8).empty;
        defer input_line.deinit(allocator);

        try term.appendStyle(&input_line, allocator, border_style);
        try input_line.appendSlice(allocator, theme.Box.vertical);
        try term.appendStyle(&input_line, allocator, .{ .fg = theme.Theme.text_cyan, .bold = true });
        try input_line.appendSlice(allocator, " > ");
        try term.appendStyle(&input_line, allocator, .{ .fg = theme.Theme.text_main });

        if (self.text.items.len == 0) {
            try term.appendStyle(&input_line, allocator, .{ .fg = theme.Theme.text_dim, .italic = true });
            try input_line.appendSlice(allocator, "What would you like to build?");
            try term.appendStyle(&input_line, allocator, .{ .fg = theme.Theme.text_cyan, .bold = true });
            try input_line.appendSlice(allocator, "▋"); // Blinking cursor
        } else {
            // İmleçli metin
            const before_cursor = self.text.items[0..self.cursor_pos];
            const after_cursor = self.text.items[self.cursor_pos..];
            try input_line.appendSlice(allocator, before_cursor);
            try term.appendStyle(&input_line, allocator, .{ .fg = theme.Theme.text_cyan, .bold = true });
            try input_line.appendSlice(allocator, "▋");
            try term.appendStyle(&input_line, allocator, .{ .fg = theme.Theme.text_main });
            try input_line.appendSlice(allocator, after_cursor);
        }

        const prompt_content_w = if (self.text.items.len == 0)
            unicode.strWidth("What would you like to build?") + 5
        else
            unicode.strWidth(self.text.items) + 5;

        var in_pad = if (width > prompt_content_w + 1) width - prompt_content_w - 1 else 1;
        try term.appendStyle(&input_line, allocator, border_style);
        while (in_pad > 0) : (in_pad -= 1) {
            try input_line.appendSlice(allocator, " ");
        }
        try input_line.appendSlice(allocator, theme.Box.vertical);
        try input_line.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try input_line.toOwnedSlice(allocator));

        // 3. Alt Durum Rozetleri: ╰─ 🟢 Claude 3.5 | 🎯 Build Mode | 🎙️ Voice: Off | 2.4k tok ─╯
        var bot_buf = std.ArrayList(u8).empty;
        defer bot_buf.deinit(allocator);

        try term.appendStyle(&bot_buf, allocator, border_style);
        try bot_buf.appendSlice(allocator, theme.Box.bottom_left);
        try bot_buf.appendSlice(allocator, theme.Box.horizontal);

        // Rozet 1: Model
        try term.appendStyle(&bot_buf, allocator, .{ .fg = theme.Theme.text_green, .bold = true });
        try bot_buf.appendSlice(allocator, " 🟢 ");
        try bot_buf.appendSlice(allocator, self.model_name);
        try term.appendStyle(&bot_buf, allocator, border_style);
        try bot_buf.appendSlice(allocator, " │");

        // Rozet 2: Mod
        try term.appendStyle(&bot_buf, allocator, .{ .fg = theme.Theme.text_yellow, .bold = true });
        try bot_buf.appendSlice(allocator, " 🎯 ");
        try bot_buf.appendSlice(allocator, self.mode_name);
        try term.appendStyle(&bot_buf, allocator, border_style);
        try bot_buf.appendSlice(allocator, " │");

        // Rozet 3: Voice
        if (self.voice_active) {
            try term.appendStyle(&bot_buf, allocator, .{ .fg = Color{ .ansi = 117 }, .bold = true });
            try bot_buf.appendSlice(allocator, " 🎙️ Voice: ON ");
        } else {
            try term.appendStyle(&bot_buf, allocator, .{ .fg = theme.Theme.text_dim });
            try bot_buf.appendSlice(allocator, " 🎙️ Voice: Off ");
        }
        try term.appendStyle(&bot_buf, allocator, border_style);
        try bot_buf.appendSlice(allocator, "│");

        // Rozet 4: Token
        var tok_str: [32]u8 = undefined;
        const tok_line = try std.fmt.bufPrint(&tok_str, " ⚡ {d} tok ", .{self.token_count});
        try term.appendStyle(&bot_buf, allocator, .{ .fg = theme.Theme.text_dim });
        try bot_buf.appendSlice(allocator, tok_line);
        try term.appendStyle(&bot_buf, allocator, border_style);

        const badge_total_w = 4 + unicode.strWidth(self.model_name) + 4 + unicode.strWidth(self.mode_name) + 16 + tok_line.len + 3;
        var b_pad = if (width > badge_total_w + 1) width - badge_total_w - 1 else 1;
        while (b_pad > 0) : (b_pad -= 1) {
            try bot_buf.appendSlice(allocator, theme.Box.horizontal);
        }
        try bot_buf.appendSlice(allocator, theme.Box.bottom_right);
        try bot_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try bot_buf.toOwnedSlice(allocator));

        return lines;
    }
};

test "input box yazma, silme ve autocomplete tetikleme" {
    var in_box = InputBox.init(std.testing.allocator);
    defer in_box.deinit();

    try in_box.insertSlice("/go");
    try std.testing.expect(in_box.show_autocomplete);

    _ = try in_box.handleKey("\n");
    try std.testing.expectEqualStrings("/goal", in_box.text.items);
}
