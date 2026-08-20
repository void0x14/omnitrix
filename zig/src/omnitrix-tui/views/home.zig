const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const layout_mod = @import("../core/layout.zig");
const footer_mod = @import("footer.zig");
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
const FooterView = footer_mod.FooterView;

const LOGO_LINES = [_][]const u8{
    " ██████╗ ███╗   ███╗███╗   ██╗██╗████████╗██████╗ ██╗██╗  ██╗",
    "██╔═══██╗████╗ ████║████╗  ██║██║╚══██╔══╝██╔══██╗██║╚██╗██╔╝",
    "██║   ██║██╔████╔██║██╔██╗ ██║██║   ██║   ██████╔╝██║ ╚███╔╝",
    "██║   ██║██║╚██╔╝██║██║╚██╗██║██║   ██║   ██╔══██╗██║ ██╔██╗",
    "╚██████╔╝██║ ╚═╝ ██║██║ ╚████║██║   ██║   ██║  ██║██║██╔╝ ██╗",
    " ╚═════╝ ╚═╝     ╚═╝╚═╝  ╚═══╝╚═╝   ╚═╝   ╚═╝  ╚═╝╚═╝╚═╝  ╚═╝",
};

pub const ComposerMode = enum { normal, multiline, shell, history, @"error" };
pub const ComposerFocus = enum { prompt, mode, history };

pub const HomeView = struct {
    pub const max_prompt_bytes: usize = 512;

    prompt: TextareaWidget,
    logo_style: Style,
    placeholder_text: []const u8,
    prompt_width: u16,
    last_frame_time: u64,
    composer_mode: ComposerMode = .normal,
    composer_focus: ComposerFocus = .prompt,
    notice_len: u8 = 0,
    notice: [160]u8 = undefined,
    footer: FooterView = FooterView.init(),

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
            .composer_mode = .normal,
            .composer_focus = .prompt,
        };
    }

    pub fn deinit(self: *HomeView) void {
        self.prompt.deinit();
    }

    pub fn setComposerMode(self: *HomeView, mode: ComposerMode) void {
        self.composer_mode = mode;
        self.composer_focus = if (mode == .history) .history else .prompt;
        self.prompt.multiline = mode == .multiline;
    }

    pub fn setNotice(self: *HomeView, text: []const u8) void {
        const length = @min(text.len, self.notice.len);
        @memcpy(self.notice[0..length], text[0..length]);
        self.notice_len = @intCast(length);
    }

    pub fn render(self: *HomeView, buf: *Buffer, terminal_width: u16, terminal_height: u16, theme: Theme) void {
        // Clear screen
        buf.fillRegion(0, 0, terminal_width, terminal_height, .{ .style = .{ .bg = theme.background } });

        // Center logo vertically
        const logo_height: u16 = @intCast(LOGO_LINES.len);
        const logo_width: u16 = 61;
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
        const subtitle = "The Omnitrix coding agent";
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
            _ = buf.writeStringBounded(inner.x + 3, inner.y + 1, self.placeholder_text, Style{ .fg = theme.prompt_placeholder, .bg = theme.background_panel }, inner.width -| 5);
            // Blinking cursor, one cell left of the placeholder
            buf.setCell(inner.x + 2, inner.y + 1, .{
                .char = .{ .char = '█' },
                .style = Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
            });
        } else {
            var text_buf: [512]u8 = undefined;
            if (text_len <= text_buf.len) {
                const text = self.prompt.gap.getText(&text_buf);
                _ = buf.writeStringBounded(inner.x + 2, inner.y + 1, text, input_style, inner.width -| 4);
                // Cursor position is display-width aware and never overwrites
                // the first byte of a UTF-8 codepoint.
                const cursor_w = buffer_mod.stringWidth(text);
                const cursor_x = inner.x + 2 + @min(cursor_w, inner.width -| 5);
                buf.setCell(cursor_x, inner.y + 1, .{
                    .char = .{ .char = '█' },
                    .style = Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
                });
            }
        }

        // Bottom border
        buf.setCell(inner.x, inner.y + 2, .{ .char = .{ .char = '╰' }, .style = border_style });
        for (1..inner.width - 1) |i| {
            buf.setCell(inner.x + @as(u16, @intCast(i)), inner.y + 2, .{ .char = .{ .char = '─' }, .style = border_style });
        }
        buf.setCell(inner.x + inner.width - 1, inner.y + 2, .{ .char = .{ .char = '╯' }, .style = border_style });

        // Compact fixture metadata keeps the main logo hierarchy intact.
        const metadata_y = prompt_y + 4;
        if (metadata_y < terminal_height -| 2) {
            const metadata = switch (self.composer_mode) {
                .normal => "[normal]  model MiMo-V2.5-Pro  provider fixture  effort balanced",
                .multiline => "[multiline]  model MiMo-V2.5-Pro  provider fixture  effort balanced",
                .shell => "[shell fixture]  model MiMo-V2.5-Pro  provider fixture  effort balanced",
                .history => "[history]  model MiMo-V2.5-Pro  provider fixture  effort balanced",
                .@"error" => "[error]  model MiMo-V2.5-Pro  provider fixture  effort balanced",
            };
            const metadata_w = buffer_mod.stringWidth(metadata);
            const metadata_x = if (metadata_w < terminal_width) (terminal_width - metadata_w) / 2 else 0;
            _ = buf.writeStringBounded(metadata_x, metadata_y, metadata, .{ .fg = theme.text_dim, .bg = theme.background }, terminal_width -| metadata_x);
        }
        if (self.notice_len > 0 and prompt_y + 5 < terminal_height -| 1) {
            _ = buf.writeStringBounded(2, prompt_y + 5, self.notice[0..self.notice_len], .{ .fg = theme.warning, .bg = theme.background }, terminal_width -| 4);
        }

        // Path/version footer and contextual controls.
        const path_y = terminal_height -| 2;
        _ = buf.writeStringBounded(1, path_y, "~/omnitrix  •  v0.1.0-ui-fixture", .{ .fg = theme.text_dim, .bg = theme.background }, terminal_width -| 2);
        const footer_y = terminal_height - 1;
        const mode = switch (self.composer_mode) {
            .normal => "normal",
            .multiline => "multiline",
            .shell => "shell",
            .history => "history",
            .@"error" => "error",
        };
        self.footer.renderContext(buf, footer_y, terminal_width, theme, .{ .route = .home, .focus = .prompt, .composer_mode = mode });
    }
};
