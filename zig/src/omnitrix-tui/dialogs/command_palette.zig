const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const layout_mod = @import("../core/layout.zig");
const box_mod = @import("../widgets/box.zig");
const gap_mod = @import("../input/gap_buffer.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const Rect = layout_mod.Rect;
const BoxWidget = box_mod.BoxWidget;
const BorderStyle = box_mod.BorderStyle;
const GapBuffer = gap_mod.GapBuffer;

pub const Command = struct {
    name: []const u8,
    label: []const u8,
    category: []const u8,
    shortcut: ?[]const u8 = null,
};

pub const CommandPalette = struct {
    allocator: std.mem.Allocator,
    commands: []const Command,
    filtered_indices: std.ArrayList(usize),
    selected: i32,
    search: GapBuffer,
    visible: bool,
    theme: Theme,

    pub fn init(allocator: std.mem.Allocator, commands: []const Command, theme: Theme) !CommandPalette {
        return .{
            .allocator = allocator,
            .commands = commands,
            .filtered_indices = .empty,
            .selected = 0,
            .search = try GapBuffer.init(allocator, 128),
            .visible = false,
            .theme = theme,
        };
    }

    pub fn deinit(self: *CommandPalette) void {
        self.filtered_indices.deinit(self.allocator);
        self.search.deinit();
    }

    pub fn show(self: *CommandPalette) void {
        self.visible = true;
        self.search.clear();
        self.selected = 0;
        self.applyFilter();
    }

    pub fn hide(self: *CommandPalette) void {
        self.visible = false;
    }

    pub fn toggle(self: *CommandPalette) void {
        if (self.visible) self.hide() else self.show();
    }

    fn applyFilter(self: *CommandPalette) void {
        self.filtered_indices.clearRetainingCapacity();
        var query_buf: [256]u8 = undefined;
        const query = if (self.search.length() <= query_buf.len)
            self.search.getText(&query_buf)
        else
            "";

        if (query.len == 0) {
            for (self.commands, 0..) |_, i| {
                self.filtered_indices.append(self.allocator, i) catch break;
            }
        } else {
            for (self.commands, 0..) |cmd, i| {
                if (fuzzyMatch(query, cmd.name) or fuzzyMatch(query, cmd.label)) {
                    self.filtered_indices.append(self.allocator, i) catch break;
                }
            }
        }
        self.selected = 0;
    }

    pub fn handleKey(self: *CommandPalette, key: struct {
        char: ?u21 = null,
        enter: bool = false,
        escape: bool = false,
        up: bool = false,
        down: bool = false,
        backspace: bool = false,
    }) ?[]const u8 {
        if (!self.visible) return null;

        if (key.escape) {
            self.hide();
            return null;
        }

        if (key.up) {
            self.selected = @max(0, self.selected - 1);
            return null;
        }
        if (key.down) {
            self.selected = @min(@as(i32, @intCast(self.filtered_indices.items.len)) - 1, self.selected + 1);
            return null;
        }

        if (key.backspace) {
            _ = self.search.deleteBackward();
            self.applyFilter();
            return null;
        }

        if (key.char) |ch| {
            if (ch >= 0x20) {
                self.search.insertCodepoint(ch) catch return null;
                self.applyFilter();
            }
            return null;
        }

        if (key.enter) {
            if (self.filtered_indices.items.len > 0 and self.selected >= 0) {
                const idx: usize = @intCast(self.selected);
                if (idx < self.filtered_indices.items.len) {
                    const cmd_idx = self.filtered_indices.items[idx];
                    const cmd = self.commands[cmd_idx];
                    self.hide();
                    return cmd.name;
                }
            }
            self.hide();
            return null;
        }

        return null;
    }

    pub fn render(self: *CommandPalette, buf: *Buffer, terminal_width: u16, terminal_height: u16) void {
        if (!self.visible) return;

        const palette_w: u16 = @min(60, terminal_width - 4);
        const palette_h: u16 = @min(20, terminal_height - 4);
        const palette_x = (terminal_width - palette_w) / 2;
        const palette_y = (terminal_height - palette_h) / 2;

        // Dim overlay
        for (0..terminal_height) |row| {
            for (0..terminal_width) |col| {
                var existing = buf.getCell(@intCast(col), @intCast(row));
                existing.style.bg = .{ .rgb = .{ .r = 0, .g = 0, .b = 0 } };
                existing.style.attr.dim = true;
                buf.setCell(@intCast(col), @intCast(row), existing);
            }
        }

        // Border
        const border_style = Style{ .fg = self.theme.border_active, .bg = self.theme.background_elevated };
        const bg_style = Style{ .fg = self.theme.text, .bg = self.theme.background_elevated };
        const box = BoxWidget.init(.single, border_style, bg_style);
        box.render(buf, .{ .x = palette_x, .y = palette_y, .width = palette_w, .height = palette_h });

        // Search input
        const input_y = palette_y + 1;
        var search_text: [256]u8 = undefined;
        const query = self.search.getText(&search_text);
        const input_style = Style{ .fg = self.theme.text, .bg = self.theme.background_panel };
        for (0..palette_w - 2) |i| {
            buf.setCell(palette_x + 1 + @as(u16, @intCast(i)), input_y, .{ .style = input_style });
        }
        _ = buf.writeStringBounded(palette_x + 2, input_y, query, input_style, palette_w -| 4);
        buf.setCell(palette_x + 2 + @as(u16, @intCast(@min(query.len, palette_w -| 5))), input_y, .{
            .char = .{ .char = '█' },
            .style = Style{ .fg = self.theme.prompt_cursor, .bg = self.theme.background_panel },
        });

        // Separator
        const sep_y = input_y + 1;
        for (0..palette_w - 2) |i| {
            buf.setCell(palette_x + 1 + @as(u16, @intCast(i)), sep_y, .{
                .char = .{ .char = '─' },
                .style = Style{ .fg = self.theme.border, .bg = self.theme.background_elevated },
            });
        }

        // Command list
        const list_y = sep_y + 1;
        const list_h = palette_h -| 4;
        const max_items = @min(self.filtered_indices.items.len, @as(usize, list_h));

        if (self.selected >= 0) {
            const sel: usize = @intCast(self.selected);
            if (sel >= max_items and max_items > 0) {
                self.selected = @intCast(max_items - 1);
            }
        }

        for (0..max_items) |i| {
            const row = list_y + @as(u16, @intCast(i));
            const cmd_idx = self.filtered_indices.items[i];
            const cmd = self.commands[cmd_idx];
            const is_selected = @as(i32, @intCast(i)) == self.selected;

            const row_style = if (is_selected)
                Style{ .fg = self.theme.background_elevated, .bg = self.theme.accent }
            else
                bg_style;

            for (0..palette_w - 2) |col| {
                buf.setCell(palette_x + 1 + @as(u16, @intCast(col)), row, .{ .style = row_style });
            }

            if (is_selected) {
                buf.setCell(palette_x + 1, row, .{ .char = .{ .char = '›' }, .style = Style{ .fg = self.theme.accent, .bg = self.theme.background_elevated } });
            }

            const cat_style = Style{
                .fg = self.theme.accent_muted,
                .bg = if (is_selected) self.theme.accent else self.theme.background_elevated,
                .attr = .{ .bold = true },
            };
            const cat_w = @as(u16, @intCast(@min(cmd.category.len + 1, palette_w -| 6)));
            _ = buf.writeStringBounded(palette_x + 3, row, cmd.category, cat_style, cat_w);

            const label_style = Style{
                .fg = self.theme.text,
                .bg = if (is_selected) self.theme.accent else self.theme.background_elevated,
            };
            const label_x = palette_x + 3 + cat_w;
            const label_w = palette_w -| 3 - cat_w;
            _ = buf.writeStringBounded(label_x, row, cmd.label, label_style, label_w);

            if (cmd.shortcut) |sc| {
                const sc_style = Style{
                    .fg = self.theme.text_muted,
                    .bg = if (is_selected) self.theme.accent else self.theme.background_elevated,
                };
                const sc_w = @as(u16, @intCast(sc.len));
                if (palette_w > sc_w + 3) {
                    _ = buf.writeStringBounded(palette_x + palette_w - 1 - sc_w, row, sc, sc_style, sc_w);
                }
            }
        }


        if (max_items == 0 and list_h > 0) {
            const empty_style = Style{ .fg = self.theme.text_dim, .bg = self.theme.background_elevated };
            _ = buf.writeStringBounded(palette_x + 2, list_y, "No commands found.", empty_style, palette_w -| 4);
        }

        // Footer hint
        const footer_y = palette_y + palette_h -| 1;
        const hint = "↑↓ navigate  Enter select  Esc cancel";
        const hint_style = Style{ .fg = self.theme.text_dim, .bg = self.theme.background_elevated };
        _ = buf.writeStringBounded(palette_x + 1, footer_y, hint, hint_style, palette_w -| 2);
    }
};

fn fuzzyMatch(query: []const u8, target: []const u8) bool {
    if (query.len == 0) return true;
    var qi: usize = 0;
    for (target) |ch| {
        if (qi < query.len and (ch == query[qi] or ch == std.ascii.toLower(query[qi]))) {
            qi += 1;
        }
    }
    return qi == query.len;
}
