//! omnitrix-tui/dialogs/command_palette.zig
//!
//! OpenCode stili Slash Komut Paleti Modalı (Command Palette Dialog).
//! Kullanıcı `/` yazdığında veya `Ctrl+K` bastığında açılır;
//! sistem komutlarını (/voice, /diff, /files, /model, /clear, /soak, /help, /quit)
//! açıklamaları ve kısayollarıyla listeleyip filtreler.

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

pub const CommandItem = struct {
    command: []const u8,
    description: []const u8,
    shortcut: []const u8,
};

pub const CommandPalette = struct {
    allocator: std.mem.Allocator,
    modal: Modal,
    commands: std.ArrayList(CommandItem),
    selected_index: usize = 0,
    is_open: bool = false,

    pub fn init(allocator: std.mem.Allocator) !CommandPalette {
        var commands = std.ArrayList(CommandItem).empty;
        errdefer commands.deinit(allocator);

        try commands.append(allocator, .{ .command = "/model", .description = "Aktif yapay zeka modelini seç", .shortcut = "Ctrl+P" });
        try commands.append(allocator, .{ .command = "/voice", .description = "Grok Sesli Modunu aç/kapat", .shortcut = "Ctrl+V" });
        try commands.append(allocator, .{ .command = "/diff", .description = "Değişiklik diff panelini aç", .shortcut = "Tab / 3" });
        try commands.append(allocator, .{ .command = "/files", .description = "Değişen dosyalar listesini aç", .shortcut = "Tab / 2" });
        try commands.append(allocator, .{ .command = "/clear", .description = "Sohbet geçmişini ve blokları temizle", .shortcut = "Ctrl+L" });
        try commands.append(allocator, .{ .command = "/soak", .description = "24/7 Performans ve sızıntı testini başlat", .shortcut = "" });
        try commands.append(allocator, .{ .command = "/help", .description = "Tüm kısayolları ve komutları göster", .shortcut = "?" });
        try commands.append(allocator, .{ .command = "/quit", .description = "Omnitrix'ten çık", .shortcut = "q / Ctrl+C" });

        return .{
            .allocator = allocator,
            .modal = Modal.init(.{
                .title = "Command Palette",
                .width_pct = 60,
                .height_pct = 50,
                .min_width = 45,
                .min_height = 12,
            }),
            .commands = commands,
            .selected_index = 0,
            .is_open = false,
        };
    }

    pub fn deinit(self: *CommandPalette) void {
        self.commands.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn open(self: *CommandPalette) void {
        self.is_open = true;
        self.selected_index = 0;
    }

    pub fn close(self: *CommandPalette) void {
        self.is_open = false;
    }

    /// Tuş olayını işler. Komut seçilirse komut metnini döner.
    pub fn handleKey(self: *CommandPalette, event: KeyEvent) ?[]const u8 {
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
                        if (self.commands.items.len > 0 and self.selected_index + 1 < self.commands.items.len) {
                            self.selected_index += 1;
                        }
                    },
                    .enter => {
                        if (self.commands.items.len > 0) {
                            const cmd = self.commands.items[self.selected_index].command;
                            self.close();
                            return cmd;
                        }
                    },
                    else => {},
                }
            },
            else => {},
        }
        return null;
    }

    /// Komut paletini modal çerçevesiyle çizer.
    pub fn render(self: *CommandPalette, screen_area: Rect, buf: *Buffer) void {
        if (!self.is_open) return;

        _ = self.modal.calculateArea(screen_area);
        const inner = self.modal.renderFrame(screen_area, buf);
        if (inner.isEmpty()) return;

        var y = inner.top();

        for (self.commands.items, 0..) |cmd, idx| {
            if (y >= inner.bottom() - 1) break;

            const is_selected = (idx == self.selected_index);
            const prefix = if (is_selected) " ❯ " else "   ";

            var line_buf: [256]u8 = undefined;
            const line_str = std.fmt.bufPrint(&line_buf, "{s}{s: <10} {s}", .{
                prefix,
                cmd.command,
                cmd.description,
            }) catch cmd.command;

            const style: Style = if (is_selected)
                .{ .fg = .bright_white, .bg = .{ .indexed = 237 }, .modifier = .{ .bold = true } }
            else
                .{ .fg = .{ .indexed = 250 } };

            _ = buf.setString(inner.left(), y, line_str, style, inner.width);

            // Kısayol Etiketi (Sağa Yaslı)
            if (cmd.shortcut.len > 0 and inner.width > 20) {
                const sc_x = inner.right() - @as(u16, @intCast(cmd.shortcut.len)) - 2;
                _ = buf.setString(sc_x, y, cmd.shortcut, .{ .fg = .{ .indexed = 244 } }, inner.width);
            }

            y += 1;
        }

        const hint_y = inner.bottom() - 1;
        _ = buf.setString(inner.left(), hint_y, " [↑/↓] Gezin  │  [Enter] Çalıştır  │  [Esc] Kapat", .{ .fg = .{ .indexed = 241 } }, inner.width);
    }
};

test "command palette: open, navigate, select" {
    var cp = try CommandPalette.init(std.testing.allocator);
    defer cp.deinit();

    cp.open();
    try std.testing.expect(cp.is_open);

    // Aşağı in
    _ = cp.handleKey(KeyEvent.special(.down, .none));
    try std.testing.expectEqual(@as(usize, 1), cp.selected_index);

    // Enter
    const chosen = cp.handleKey(KeyEvent.special(.enter, .none));
    try std.testing.expect(chosen != null);
    try std.testing.expectEqualStrings("/voice", chosen.?);
    try std.testing.expect(!cp.is_open);
}
