const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const layout_mod = @import("../core/layout.zig");
const textarea_mod = @import("../widgets/textarea.zig");
const text_mod = @import("../widgets/text.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const Rect = layout_mod.Rect;
const TextareaWidget = textarea_mod.TextareaWidget;
const TextWidget = text_mod.TextWidget;

const LOGO_LINES = [_][]const u8{
    "  ██████╗ ███╗   ███╗██╗███╗   ██╗████████╗██╗██╗  ██╗",
    " ██╔═══██╗████╗ ████║██║████╗  ██║╚══██╔══╝██║╚██╗██╔╝",
    " ██║   ██║██╔████╔██║██║██╔██╗ ██║   ██║   ██║ ╚███╔╝ ",
    " ██║   ██║██║╚██╔╝██║██║██║╚██╗██║   ██║   ██║ ██╔██╗ ",
    " ╚██████╔╝██║ ╚═╝ ██║██║██║ ╚████║   ██║   ██║██╔╝ ██╗",
    "  ╚═════╝ ╚═╝     ╚═╝╚═╝╚═╝  ╚═══╝   ╚═╝   ╚═╝╚═╝  ╚═╝",
};

pub const HomeView = struct {
    prompt: TextareaWidget,
    logo_style: Style,
    placeholder_text: []const u8,
    prompt_width: u16,
    last_frame_time: u64,

    pub fn init(allocator: std.mem.Allocator, theme: Theme) !HomeView {
        var prompt = try TextareaWidget.init(
            allocator,
            Style{ .fg = theme.text, .bg = theme.background_panel },
            Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
        );
        prompt.placeholder = "Ask me anything... (Tab for shell mode)";
        prompt.placeholder_style = Style{ .fg = theme.prompt_placeholder, .bg = theme.background_panel };

        return .{
            .prompt = prompt,
            .logo_style = Style{ .fg = theme.accent, .bg = theme.background },
            .placeholder_text = "Ask me anything...",
            .prompt_width = 75,
            .last_frame_time = 0,
        };
    }

    pub fn deinit(self: *HomeView) void {
        self.prompt.deinit();
    }

    pub fn render(self: *HomeView, buf: *Buffer, terminal_width: u16, terminal_height: u16, theme: Theme) void {
        // Clear screen
        buf.fillRegion(0, 0, terminal_width, terminal_height, .{ .style = .{ .bg = theme.background } });

        // Center logo vertically
        const logo_height: u16 = @intCast(LOGO_LINES.len);
        const logo_width: u16 = 58;
        const prompt_area_height: u16 = 4; // prompt box + padding

        const total_content_height = logo_height + 2 + prompt_area_height;
        const start_y = if (total_content_height < terminal_height)
            (terminal_height - total_content_height) / 2
        else
            0;

        // Render logo
        const logo_x = if (logo_width < terminal_width) (terminal_width - logo_width) / 2 else 0;
        for (LOGO_LINES, 0..) |line, i| {
            _ = buf.writeStringBounded(logo_x, start_y + @as(u16, @intCast(i)), line, self.logo_style, terminal_width - logo_x);
        }

        // Subtitle
        const subtitle_y = start_y + logo_height + 1;
        const subtitle = "The open source coding agent";
        const subtitle_style = Style{ .fg = theme.text_muted, .bg = theme.background };
        const sub_w = buffer_mod.stringWidth(subtitle);
        const sub_x = if (sub_w < terminal_width) (terminal_width - sub_w) / 2 else 0;
        _ = buf.writeStringBounded(sub_x, subtitle_y, subtitle, subtitle_style, terminal_width);

        // Prompt area
        const prompt_y = subtitle_y + 2;
        const pw = @min(self.prompt_width, terminal_width - 4);
        const prompt_x = if (pw < terminal_width) (terminal_width - pw) / 2 else 2;

        // Prompt border
        const border_style = Style{ .fg = theme.border, .bg = theme.background_panel };
        const inner = Rect{
            .x = prompt_x,
            .y = prompt_y,
            .width = pw,
            .height = 3,
        };

        // Top border
        buf.setCell(inner.x, inner.y, .{ .char = .{ .char = '╭' }, .style = border_style });
        for (1..inner.width - 1) |i| {
            buf.setCell(inner.x + @as(u16, @intCast(i)), inner.y, .{ .char = .{ .char = '─' }, .style = border_style });
        }
        buf.setCell(inner.x + inner.width - 1, inner.y, .{ .char = .{ .char = '╮' }, .style = border_style });

        // Content line
        buf.setCell(inner.x, inner.y + 1, .{ .char = .{ .char = '│' }, .style = border_style });
        const input_style = Style{ .fg = theme.text, .bg = theme.background_panel };
        for (1..inner.width - 1) |i| {
            buf.setCell(inner.x + @as(u16, @intCast(i)), inner.y + 1, .{ .style = input_style });
        }
        buf.setCell(inner.x + inner.width - 1, inner.y + 1, .{ .char = .{ .char = '│' }, .style = border_style });

        // Write prompt text or placeholder
        const text_len = self.prompt.gap.length();
        if (text_len == 0) {
            _ = buf.writeStringBounded(inner.x + 2, inner.y + 1, self.placeholder_text, Style{ .fg = theme.prompt_placeholder, .bg = theme.background_panel }, inner.width -| 4);
            // Blinking cursor
            buf.setCell(inner.x + 2, inner.y + 1, .{
                .char = .{ .char = '█' },
                .style = Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
            });
        } else {
            var text_buf: [512]u8 = undefined;
            const text = self.prompt.gap.getText(&text_buf);
            _ = buf.writeStringBounded(inner.x + 2, inner.y + 1, text, input_style, inner.width -| 4);
            // Cursor at end
            const cursor_x = inner.x + 2 + @min(@as(u16, @intCast(text_len)), inner.width -| 5);
            buf.setCell(cursor_x, inner.y + 1, .{
                .char = .{ .char = '█' },
                .style = Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
            });
        }

        // Bottom border
        buf.setCell(inner.x, inner.y + 2, .{ .char = .{ .char = '╰' }, .style = border_style });
        for (1..inner.width - 1) |i| {
            buf.setCell(inner.x + @as(u16, @intCast(i)), inner.y + 2, .{ .char = .{ .char = '─' }, .style = border_style });
        }
        buf.setCell(inner.x + inner.width - 1, inner.y + 2, .{ .char = .{ .char = '╯' }, .style = border_style });

        // Footer
        const footer_y = terminal_height - 1;
        const footer_style = Style{ .fg = theme.text_dim, .bg = theme.background };
        const shortcuts = "Ctrl+P: Commands  Tab: Shell mode  Ctrl+K: Clear";
        const sc_w = buffer_mod.stringWidth(shortcuts);
        const sc_x = if (sc_w < terminal_width) (terminal_width - sc_w) / 2 else 0;
        _ = buf.writeStringBounded(sc_x, footer_y, shortcuts, footer_style, terminal_width);
    }
};
