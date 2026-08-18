const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const layout_mod = @import("../core/layout.zig");
const box_mod = @import("../widgets/box.zig");
const list_mod = @import("../widgets/list.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const Rect = layout_mod.Rect;
const BoxWidget = box_mod.BoxWidget;
const BorderStyle = box_mod.BorderStyle;

pub const DialogType = enum {
    alert,
    confirm,
    select,
    prompt,
    none,
};

pub const DialogState = struct {
    type_: DialogType = .none,
    title: [128]u8 = undefined,
    title_len: usize = 0,
    message: [512]u8 = undefined,
    message_len: usize = 0,
    select_items: []const []const u8 = &.{},
    selected_index: i32 = 0,
    scroll_offset: u16 = 0,
    prompt_value: [256]u8 = undefined,
    prompt_len: usize = 0,
    confirmed: bool = false,
    visible: bool = false,

    pub fn hide(self: *DialogState) void {
        self.visible = false;
        self.type_ = .none;
    }

    pub fn showTitle(self: *DialogState, title: []const u8) void {
        const len = @min(title.len, self.title.len);
        @memcpy(self.title[0..len], title[0..len]);
        self.title_len = len;
    }

    pub fn showMessage(self: *DialogState, msg: []const u8) void {
        const len = @min(msg.len, self.message.len);
        @memcpy(self.message[0..len], msg[0..len]);
        self.message_len = len;
    }
};

pub const ModalDialog = struct {
    state: DialogState,
    theme: Theme,

    pub fn init(theme: Theme) ModalDialog {
        return .{
            .state = .{},
            .theme = theme,
        };
    }

    pub fn showAlert(self: *ModalDialog, title: []const u8, message: []const u8) void {
        self.state.type_ = .alert;
        self.state.showTitle(title);
        self.state.showMessage(message);
        self.state.visible = true;
    }

    pub fn showConfirm(self: *ModalDialog, title: []const u8, message: []const u8) void {
        self.state.type_ = .confirm;
        self.state.showTitle(title);
        self.state.showMessage(message);
        self.state.confirmed = false;
        self.state.visible = true;
    }

    pub fn showSelect(self: *ModalDialog, title: []const u8, items: []const []const u8) void {
        self.state.type_ = .select;
        self.state.showTitle(title);
        self.state.select_items = items;
        self.state.selected_index = 0;
        self.state.scroll_offset = 0;
        self.state.visible = true;
    }

    pub fn showPrompt(self: *ModalDialog, title: []const u8, message: []const u8) void {
        self.state.type_ = .prompt;
        self.state.showTitle(title);
        self.state.showMessage(message);
        self.state.prompt_len = 0;
        self.state.visible = true;
    }

    pub fn handleKey(self: *ModalDialog, key: struct { char: ?u21 = null, enter: bool = false, escape: bool = false, up: bool = false, down: bool = false }) bool {
        if (!self.state.visible) return false;

        if (key.escape) {
            self.state.hide();
            return true;
        }

        switch (self.state.type_) {
            .alert => {
                if (key.enter) {
                    self.state.hide();
                    return true;
                }
            },
            .confirm => {
                if (key.enter) {
                    self.state.confirmed = true;
                    self.state.hide();
                    return true;
                }
                if (key.char) |ch| {
                    if (ch == 'y' or ch == 'Y') {
                        self.state.confirmed = true;
                        self.state.hide();
                        return true;
                    }
                    if (ch == 'n' or ch == 'N') {
                        self.state.confirmed = false;
                        self.state.hide();
                        return true;
                    }
                }
            },
            .select => {
                if (key.up) {
                    self.state.selected_index = @max(0, self.state.selected_index - 1);
                    return true;
                }
                if (key.down) {
                    self.state.selected_index = @min(
                        @as(i32, @intCast(self.state.select_items.len)) - 1,
                        self.state.selected_index + 1,
                    );
                    return true;
                }
                if (key.enter) {
                    self.state.confirmed = true;
                    self.state.hide();
                    return true;
                }
            },
            .prompt => {
                if (key.enter) {
                    self.state.confirmed = true;
                    self.state.hide();
                    return true;
                }
                if (key.char) |ch| {
                    if (self.state.prompt_len < self.state.prompt_value.len) {
                        self.state.prompt_value[self.state.prompt_len] = @intCast(ch);
                        self.state.prompt_len += 1;
                    }
                    return true;
                }
            },
            .none => {},
        }
        return false;
    }

    /// Get the selected item value (for select dialogs)
    pub fn selectedValue(self: ModalDialog) ?[]const u8 {
        if (self.state.type_ != .select) return null;
        if (self.state.selected_index < 0) return null;
        const idx: usize = @intCast(self.state.selected_index);
        if (idx >= self.state.select_items.len) return null;
        return self.state.select_items[idx];
    }

    /// Get the prompt input value
    pub fn promptValue(self: ModalDialog) []const u8 {
        return self.state.prompt_value[0..self.state.prompt_len];
    }

    /// Render the dialog as a centered overlay
    pub fn render(self: ModalDialog, buf: *Buffer, terminal_width: u16, terminal_height: u16) void {
        if (!self.state.visible) return;

        const title = self.state.title[0..self.state.title_len];
        const message = self.state.message[0..self.state.message_len];

        // Dialog dimensions
        const dialog_w: u16 = @min(60, terminal_width - 4);
        const dialog_h: u16 = switch (self.state.type_) {
            .alert, .confirm => 7,
            .select => @intCast(@min(@as(u16, @intCast(self.state.select_items.len)) + 6, terminal_height - 4)),
            .prompt => 8,
            .none => 0,
        };

        const dialog_x = (terminal_width - dialog_w) / 2;
        const dialog_y = (terminal_height - dialog_h) / 2;

        // Dim overlay (darken background)
        for (0..terminal_height) |row| {
            for (0..terminal_width) |col| {
                var existing = buf.getCell(@intCast(col), @intCast(row));
                existing.style.bg = .{ .rgb = .{ .r = 0, .g = 0, .b = 0 } };
                existing.style.attr.dim = true;
                buf.setCell(@intCast(col), @intCast(row), existing);
            }
        }

        // Dialog box
        const dialog_style = Style{ .fg = self.theme.text, .bg = self.theme.background_elevated };
        const border_style = Style{ .fg = self.theme.border_active, .bg = self.theme.background_elevated };
        const title_style = Style{ .fg = self.theme.accent, .bg = self.theme.background_elevated, .attr = .{ .bold = true } };

        const dialog = BoxWidget.init(.single, border_style, dialog_style)
            .withTitle(title, title_style);
        dialog.render(buf, .{ .x = dialog_x, .y = dialog_y, .width = dialog_w, .height = dialog_h });

        const inner = dialog.innerRect(.{ .x = dialog_x, .y = dialog_y, .width = dialog_w, .height = dialog_h });

        // Message
        if (message.len > 0) {
            _ = buf.writeStringBounded(inner.x + 1, inner.y, message, dialog_style, inner.width -| 2);
        }

        // Type-specific rendering
        switch (self.state.type_) {
            .alert, .confirm => {
                // Hint at bottom
                const hint = if (self.state.type_ == .alert) "Press Enter to close" else "y/n";
                const hint_style = Style{ .fg = self.theme.text_muted, .bg = self.theme.background_elevated };
                _ = buf.writeStringBounded(inner.x + 1, inner.y + inner.height -| 1, hint, hint_style, inner.width -| 2);
            },
            .select => {
                // List items
                var list = list_mod.ListWidget.init(
                    &.{},
                    Style{ .fg = self.theme.text, .bg = self.theme.background_elevated },
                    Style{ .fg = self.theme.background_elevated, .bg = self.theme.accent },
                );
                // Convert items to ListItems
                var items_buf: [64]list_mod.ListItem = undefined;
                const count = @min(self.state.select_items.len, items_buf.len);
                for (0..count) |i| {
                    items_buf[i] = .{
                        .label = self.state.select_items[i],
                        .style = Style{ .fg = self.theme.text, .bg = self.theme.background_elevated },
                    };
                }
                list.items = items_buf[0..count];
                list.selected = self.state.selected_index;
                list.render(buf, .{
                    .x = inner.x,
                    .y = inner.y + 1,
                    .width = inner.width,
                    .height = inner.height -| 2,
                }, null);
            },
            .prompt => {
                // Input field
                const input_style = Style{ .fg = self.theme.text, .bg = self.theme.background_panel };
                const cursor_style = Style{ .fg = self.theme.prompt_cursor, .bg = self.theme.background_panel };
                const field_rect = Rect{
                    .x = inner.x + 1,
                    .y = inner.y + 2,
                    .width = inner.width -| 2,
                    .height = 1,
                };
                // Fill input area
                for (0..field_rect.width) |i| {
                    buf.setCell(field_rect.x + @as(u16, @intCast(i)), field_rect.y, .{ .style = input_style });
                }
                // Write value
                const val = self.state.prompt_value[0..self.state.prompt_len];
                _ = buf.writeStringBounded(field_rect.x, field_rect.y, val, input_style, field_rect.width);
                // Cursor
                const cursor_x = field_rect.x + @min(@as(u16, @intCast(self.state.prompt_len)), field_rect.width - 1);
                buf.setCell(cursor_x, field_rect.y, .{
                    .char = .{ .char = '█' },
                    .style = cursor_style,
                });
                // Hint
                const hint_style = Style{ .fg = self.theme.text_muted, .bg = self.theme.background_elevated };
                _ = buf.writeStringBounded(inner.x + 1, inner.y + inner.height -| 1, "Enter to confirm, Esc to cancel", hint_style, inner.width -| 2);
            },
            .none => {},
        }
    }
};
