//! omnitrix-tui/editor/prompt_editor.zig
//!
//! OpenCode stili interaktif çok satırlı Prompt Düzenleyici bileşeni.
//! GapBuffer çekirdeğini kullanır; imleç gezinimi, geçmiş (history),
//! slash komutları (`/`) ve dosya etiketleme (`@`) tetikleyicilerini yönetir.

const std = @import("std");
const gap_buffer_mod = @import("gap_buffer.zig");
const keys_mod = @import("../input/keys.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const cell_mod = @import("../core/cell.zig");

pub const GapBuffer = gap_buffer_mod.GapBuffer;
pub const KeyEvent = keys_mod.KeyEvent;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Style = cell_mod.Style;

pub const PromptEditor = struct {
    allocator: std.mem.Allocator,
    buffer: GapBuffer,
    history: std.ArrayList([]u8),
    history_index: ?usize = null,
    saved_draft: ?[]u8 = null,

    is_focused: bool = true,
    prompt_symbol: []const u8 = "❯ ",
    placeholder: []const u8 = "Komut veya mesajınızı yazın... (/ komutlar, @ dosyalar)",
    cursor_visible: bool = true,

    // Slash ve Mention tetikleyicileri
    show_slash_popup: bool = false,
    show_mention_popup: bool = false,

    pub fn init(allocator: std.mem.Allocator) !PromptEditor {
        const gb = try GapBuffer.init(allocator);
        return .{
            .allocator = allocator,
            .buffer = gb,
            .history = std.ArrayList([]u8).empty,
            .history_index = null,
            .saved_draft = null,
            .is_focused = true,
            .prompt_symbol = "❯ ",
            .placeholder = "Komut veya mesajınızı yazın... (/ komutlar, @ dosyalar)",
            .cursor_visible = true,
            .show_slash_popup = false,
            .show_mention_popup = false,
        };
    }

    pub fn deinit(self: *PromptEditor) void {
        self.buffer.deinit();
        for (self.history.items) |h| self.allocator.free(h);
        self.history.deinit(self.allocator);
        if (self.saved_draft) |d| self.allocator.free(d);
        self.* = undefined;
    }

    /// Klavyeden gelen tuş olayını işler.
    /// Eğer Enter tuşuna basılırsa ve mesaj gönderilirse, gönderilen metni döner.
    pub fn handleKey(self: *PromptEditor, event: KeyEvent) !?[]u8 {
        switch (event.code) {
            .char => |c| {
                if (event.modifiers.ctrl) {
                    return try self.handleCtrlKey(c);
                } else if (event.modifiers.alt) {
                    try self.handleAltKey(c);
                } else {
                    try self.buffer.insertChar(c);
                    self.checkTriggers();
                }
            },
            .special => |s| {
                switch (s) {
                    .enter => {
                        if (event.modifiers.shift) {
                            // Shift+Enter: Yeni satır ekle
                            try self.buffer.insertChar('\n');
                        } else {
                            // Normal Enter: Mesajı gönder
                            return try self.submit();
                        }
                    },
                    .backspace => {
                        _ = self.buffer.backspace();
                        self.checkTriggers();
                    },
                    .delete => {
                        _ = self.buffer.delete();
                        self.checkTriggers();
                    },
                    .left => {
                        const cur = self.buffer.cursor();
                        if (cur > 0) self.buffer.moveCursorTo(cur - 1);
                    },
                    .right => {
                        const cur = self.buffer.cursor();
                        if (cur < self.buffer.len()) self.buffer.moveCursorTo(cur + 1);
                    },
                    .home => {
                        self.buffer.moveCursorTo(0);
                    },
                    .end => {
                        self.buffer.moveCursorTo(self.buffer.len());
                    },
                    .up => {
                        try self.historyUp();
                    },
                    .down => {
                        try self.historyDown();
                    },
                    .escape => {
                        self.show_slash_popup = false;
                        self.show_mention_popup = false;
                    },
                    else => {},
                }
            },
        }
        return null;
    }

    /// Mesajı gönderir, geçmişe ekler ve düzenleyiciyi temizler.
    pub fn submit(self: *PromptEditor) !?[]u8 {
        if (self.buffer.len() == 0) return null;

        const text = try self.buffer.toString(self.allocator);
        self.buffer.clear();
        self.history_index = null;
        self.show_slash_popup = false;
        self.show_mention_popup = false;

        // Geçmişe ekle
        try self.history.append(self.allocator, try self.allocator.dupe(u8, text));
        return text;
    }

    /// 2D Hücre Tamponu üzerine Prompt Düzenleyiciyi çizer.
    pub fn render(self: *const PromptEditor, area: Rect, buf: *Buffer) void {
        if (area.isEmpty()) return;

        const border_style = Style{ .fg = .{ .indexed = 238 } };
        const text_style = Style{ .fg = .bright_white };
        const prompt_style = Style{ .fg = .{ .indexed = 39 }, .modifier = .{ .bold = true } };
        const placeholder_style = Style{ .fg = .{ .indexed = 242 } };

        // Üst ayırıcı çizgi
        var x = area.left();
        while (x < area.right()) : (x += 1) {
            if (buf.getMut(x, area.top())) |c| {
                c.setSymbol("─", 1);
                c.setStyle(border_style);
            }
        }

        const input_y = area.top() + 1;
        if (input_y >= area.bottom()) return;

        // Prompt sembolü (❯ )
        const prompt_w = buf.setString(area.left() + 1, input_y, self.prompt_symbol, prompt_style, area.width);

        const text_x = area.left() + 1 + prompt_w;
        const max_text_w = if (area.width > prompt_w + 3) area.width - prompt_w - 3 else 0;

        if (self.buffer.len() == 0) {
            // Placeholder bas
            _ = buf.setString(text_x, input_y, self.placeholder, placeholder_style, max_text_w);
            if (self.is_focused and self.cursor_visible) {
                if (buf.getMut(text_x, input_y)) |c| {
                    c.setSymbol("▋", 1);
                    c.setStyle(.{ .fg = .{ .indexed = 39 } });
                }
            }
        } else {
            // Metni ve imleci bas
            const cur_line_col = self.buffer.getCursorLineCol();
            const text_str = self.buffer.toString(self.allocator) catch "";
            defer self.allocator.free(text_str);

            var it = std.mem.splitScalar(u8, text_str, '\n');
            var line_idx: u16 = 0;
            while (it.next()) |line| : (line_idx += 1) {
                const draw_y = input_y + line_idx;
                if (draw_y >= area.bottom()) break;

                _ = buf.setString(text_x, draw_y, line, text_style, max_text_w);

                // İmleci çiz
                if (self.is_focused and self.cursor_visible and line_idx == cur_line_col.line) {
                    const cur_screen_x = text_x + @as(u16, @intCast(cur_line_col.col));
                    if (cur_screen_x < area.right()) {
                        if (buf.getMut(cur_screen_x, draw_y)) |c| {
                            c.setSymbol("▋", 1);
                            c.setStyle(.{ .fg = .{ .indexed = 39 } });
                        }
                    }
                }
            }
        }
    }

    fn handleCtrlKey(self: *PromptEditor, c: u21) !?[]u8 {
        switch (c) {
            'a' => self.buffer.moveCursorTo(0), // Satır başı
            'e' => self.buffer.moveCursorTo(self.buffer.len()), // Satır sonu
            'k' => {
                // İmleçten satır sonuna kadar sil
                const cur = self.buffer.cursor();
                self.buffer.moveCursorTo(self.buffer.len());
                while (self.buffer.cursor() > cur) {
                    _ = self.buffer.backspace();
                }
            },
            'u' => {
                // Satır başından imlece kadar sil
                while (self.buffer.cursor() > 0) {
                    _ = self.buffer.backspace();
                }
            },
            'l' => self.buffer.clear(),
            else => {},
        }
        return null;
    }

    fn handleAltKey(self: *PromptEditor, c: u21) !void {
        switch (c) {
            'b' => {
                // Kelime geri
                const cur = self.buffer.cursor();
                if (cur > 0) {
                    var new_pos = cur - 1;
                    while (new_pos > 0 and self.buffer.buffer[new_pos] == ' ') : (new_pos -= 1) {}
                    while (new_pos > 0 and self.buffer.buffer[new_pos] != ' ') : (new_pos -= 1) {}
                    self.buffer.moveCursorTo(new_pos);
                }
            },
            'f' => {
                // Kelime ileri
                const cur = self.buffer.cursor();
                const total = self.buffer.len();
                if (cur < total) {
                    var new_pos = cur;
                    while (new_pos < total and self.buffer.buffer[new_pos] != ' ') : (new_pos += 1) {}
                    while (new_pos < total and self.buffer.buffer[new_pos] == ' ') : (new_pos += 1) {}
                    self.buffer.moveCursorTo(new_pos);
                }
            },
            else => {},
        }
    }

    fn historyUp(self: *PromptEditor) !void {
        if (self.history.items.len == 0) return;

        if (self.history_index == null) {
            if (self.saved_draft) |d| self.allocator.free(d);
            self.saved_draft = try self.buffer.toString(self.allocator);
            self.history_index = self.history.items.len - 1;
        } else if (self.history_index.? > 0) {
            self.history_index.? -= 1;
        }

        if (self.history_index) |idx| {
            self.buffer.clear();
            try self.buffer.insertSlice(self.history.items[idx]);
        }
    }

    fn historyDown(self: *PromptEditor) !void {
        if (self.history_index == null) return;

        if (self.history_index.? + 1 < self.history.items.len) {
            self.history_index.? += 1;
            self.buffer.clear();
            try self.buffer.insertSlice(self.history.items[self.history_index.?]);
        } else {
            self.history_index = null;
            self.buffer.clear();
            if (self.saved_draft) |draft| {
                try self.buffer.insertSlice(draft);
                self.allocator.free(draft);
                self.saved_draft = null;
            }
        }
    }

    fn checkTriggers(self: *PromptEditor) void {
        const text = self.buffer.toString(self.allocator) catch return;
        defer self.allocator.free(text);

        if (std.mem.startsWith(u8, text, "/")) {
            self.show_slash_popup = true;
        } else {
            self.show_slash_popup = false;
        }

        if (std.mem.indexOf(u8, text, "@") != null) {
            self.show_mention_popup = true;
        } else {
            self.show_mention_popup = false;
        }
    }
};

// -----------------------------------------------------------------------------
// Unit Testler
// -----------------------------------------------------------------------------

test "prompt editor: typing, submit, history navigation" {
    var editor = try PromptEditor.init(std.testing.allocator);
    defer editor.deinit();

    // 'l', 's' yaz
    _ = try editor.handleKey(KeyEvent.char('l', .none));
    _ = try editor.handleKey(KeyEvent.char('s', .none));

    const submitted = try editor.handleKey(KeyEvent.special(.enter, .none));
    try std.testing.expect(submitted != null);
    defer std.testing.allocator.free(submitted.?);
    try std.testing.expectEqualStrings("ls", submitted.?);

    // Editör temizlenmiş olmalı
    try std.testing.expectEqual(@as(usize, 0), editor.buffer.len());

    // Geçmişte 'ls' olmalı (Up tuşu)
    _ = try editor.handleKey(KeyEvent.special(.up, .none));
    const hist_text = try editor.buffer.toString(std.testing.allocator);
    defer std.testing.allocator.free(hist_text);
    try std.testing.expectEqualStrings("ls", hist_text);
}
