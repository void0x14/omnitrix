//! omnitrix-tui/dialogs/model_selector.zig
//!
//! OpenCode stili interaktif Model Seçici Modalı (Model Selector Dialog).
//! Kullanıcının klavye ok tuşları (`↑`/`↓`), arama filtresi ve `Enter` ile
//! aktif LLM modelini (Anthropic, OpenAI, xAI Grok, Ollama vb.) değiştirmesini sağlar.

const std = @import("std");
const modal_mod = @import("modal.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const cell_mod = @import("../core/cell.zig");
const keys_mod = @import("../input/keys.zig");

pub const Modal = modal_mod.Modal;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Style = cell_mod.Style;
pub const KeyEvent = keys_mod.KeyEvent;

pub const ModelEntry = struct {
    provider: []const u8,
    name: []const u8,
    context_window: []const u8,
    badge: []const u8,
    is_active: bool = false,
};

pub const ModelSelector = struct {
    allocator: std.mem.Allocator,
    modal: Modal,
    models: std.ArrayList(ModelEntry),
    selected_index: usize = 0,
    search_query: std.ArrayList(u8),
    is_open: bool = false,

    pub fn init(allocator: std.mem.Allocator) !ModelSelector {
        var models = std.ArrayList(ModelEntry).empty;
        errdefer models.deinit(allocator);

        // Standart Model Kataloğu
        try models.append(allocator, .{ .provider = "Anthropic", .name = "Claude 3.5 Sonnet", .context_window = "200k", .badge = "Recommended", .is_active = true });
        try models.append(allocator, .{ .provider = "xAI", .name = "Grok 2.0 (Beta)", .context_window = "128k", .badge = "Fast", .is_active = false });
        try models.append(allocator, .{ .provider = "OpenAI", .name = "GPT-4o", .context_window = "128k", .badge = "Standard", .is_active = false });
        try models.append(allocator, .{ .provider = "Google", .name = "Gemini 2.0 Flash", .context_window = "1M", .badge = "Speed", .is_active = false });
        try models.append(allocator, .{ .provider = "Local", .name = "Ollama / DeepSeek R1", .context_window = "64k", .badge = "Private", .is_active = false });

        return .{
            .allocator = allocator,
            .modal = Modal.init(.{
                .title = "Select Active Model",
                .width_pct = 65,
                .height_pct = 55,
                .min_width = 50,
                .min_height = 12,
            }),
            .models = models,
            .selected_index = 0,
            .search_query = std.ArrayList(u8).empty,
            .is_open = false,
        };
    }

    pub fn deinit(self: *ModelSelector) void {
        self.models.deinit(self.allocator);
        self.search_query.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn open(self: *ModelSelector) void {
        self.is_open = true;
        self.search_query.clearRetainingCapacity();
    }

    pub fn close(self: *ModelSelector) void {
        self.is_open = false;
    }

    /// Tuş olayını işler. Model seçilirse seçilen model adını döner.
    pub fn handleKey(self: *ModelSelector, event: KeyEvent) ?[]const u8 {
        if (!self.is_open) return null;

        switch (event.code) {
            .special => |s| {
                switch (s) {
                    .escape => {
                        self.close();
                        return null;
                    },
                    .up => {
                        if (self.selected_index > 0) self.selected_index -= 1;
                    },
                    .down => {
                        if (self.models.items.len > 0 and self.selected_index + 1 < self.models.items.len) {
                            self.selected_index += 1;
                        }
                    },
                    .enter => {
                        if (self.models.items.len > 0) {
                            for (self.models.items, 0..) |*m, i| {
                                m.is_active = (i == self.selected_index);
                            }
                            const chosen = self.models.items[self.selected_index].name;
                            self.close();
                            return chosen;
                        }
                    },
                    .backspace => {
                        if (self.search_query.items.len > 0) {
                            _ = self.search_query.pop();
                        }
                    },
                    else => {},
                }
            },
            .char => |c| {
                if (c >= 0x20 and c <= 0x7E) {
                    var utf8_buf: [4]u8 = undefined;
                    const n = std.unicode.utf8Encode(c, &utf8_buf) catch 0;
                    if (n > 0) {
                        self.search_query.appendSlice(self.allocator, utf8_buf[0..n]) catch {};
                    }
                }
            },
        }
        return null;
    }

    /// Modalı ekran üzerine çizer.
    pub fn render(self: *ModelSelector, screen_area: Rect, buf: *Buffer) void {
        if (!self.is_open) return;

        _ = self.modal.calculateArea(screen_area);
        const inner = self.modal.renderFrame(screen_area, buf);
        if (inner.isEmpty()) return;

        var y = inner.top();

        // 1. Arama Çubuğu
        var search_buf: [128]u8 = undefined;
        const search_str = if (self.search_query.items.len > 0)
            std.fmt.bufPrint(&search_buf, "🔍 {s}▋", .{self.search_query.items}) catch "🔍 "
        else
            "🔍 Model ara... (DeepSeek, Claude, GPT, Grok)";
        _ = buf.setString(inner.left(), y, search_str, .{ .fg = .{ .indexed = 248 } }, inner.width);
        y += 2;

        // 2. Model Listesi
        for (self.models.items, 0..) |m, idx| {
            if (y >= inner.bottom() - 1) break;

            const is_selected = (idx == self.selected_index);
            const sel_prefix = if (is_selected) " ❯ " else "   ";
            const active_mark = if (m.is_active) " [Active]" else "";

            var line_buf: [256]u8 = undefined;
            const line_str = std.fmt.bufPrint(&line_buf, "{s}{s} · {s} ({s}){s}", .{
                sel_prefix,
                m.provider,
                m.name,
                m.context_window,
                active_mark,
            }) catch m.name;

            const line_style: Style = if (is_selected)
                .{ .fg = .bright_white, .bg = .{ .indexed = 237 }, .modifier = .{ .bold = true } }
            else if (m.is_active)
                .{ .fg = .{ .indexed = 39 }, .modifier = .{ .bold = true } }
            else
                .{ .fg = .{ .indexed = 250 } };

            _ = buf.setString(inner.left(), y, line_str, line_style, inner.width);
            y += 1;
        }

        // Alt İpucu
        const hint_y = inner.bottom() - 1;
        const hint = " [↑/↓] Gezin  │  [Enter] Seç  │  [Esc] Kapat";
        _ = buf.setString(inner.left(), hint_y, hint, .{ .fg = .{ .indexed = 241 } }, inner.width);
    }
};

test "model selector: open, navigate, select" {
    var ms = try ModelSelector.init(std.testing.allocator);
    defer ms.deinit();

    ms.open();
    try std.testing.expect(ms.is_open);

    // Aşağı ok
    _ = ms.handleKey(KeyEvent.special(.down, .none));
    try std.testing.expectEqual(@as(usize, 1), ms.selected_index);

    // Enter
    const chosen = ms.handleKey(KeyEvent.special(.enter, .none));
    try std.testing.expect(chosen != null);
    try std.testing.expectEqualStrings("Grok 2.0 (Beta)", chosen.?);
    try std.testing.expect(!ms.is_open);
}
