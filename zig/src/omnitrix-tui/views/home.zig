const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const footer_mod = @import("footer.zig");
const textarea_mod = @import("../widgets/textarea.zig");
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const TextareaWidget = textarea_mod.TextareaWidget;
const FooterView = footer_mod.FooterView;

const LOGO_LINES = [_][]const u8{
    "███  █ █  █ ██ ███  ████ ███  ███  █ ██",
    "█  █ ████ ████  ██  ██ █  █ ██  ██ ███",
    "███  █  █ ██ █ ███  ████ ███  ██  █ ██",
};

fn homeHint(mode: ComposerMode) []const u8 {
    return switch (mode) {
        .normal => "Enter send   Shift+Enter multiline   Tab shell   Ctrl+R history",
        .multiline => "Enter send   Shift+Enter newline   Tab normal   Esc cancel",
        .shell => "Tab normal   Enter shell fixture   Esc cancel",
        .history => "Up/Down history fixture   Esc close",
        .@"error" => "Esc dismiss   Ctrl+K clear   edit prompt",
    };
}

pub const ComposerMode = enum { normal, multiline, shell, history, @"error" };
pub const ComposerFocus = enum { prompt, mode, history };
pub const NoticeTone = enum { info, success, warning, @"error" };

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
    notice_tone: NoticeTone = .warning,
    footer: FooterView = FooterView.init(),

    pub fn init(allocator: std.mem.Allocator, theme: Theme) !HomeView {
        var prompt = try TextareaWidget.init(
            allocator,
            Style{ .fg = theme.text, .bg = theme.background_panel },
            Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
        );
        prompt.placeholder = "Ask anything... \"Fix a TODO in the codebase\"";
        prompt.placeholder_style = Style{ .fg = theme.prompt_placeholder, .bg = theme.background_panel };

        return .{
            .prompt = prompt,
            .logo_style = Style{ .fg = theme.accent, .bg = theme.background },
            .placeholder_text = "Ask anything... \"Fix a TODO in the codebase\"",
            .prompt_width = 74,
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
        self.setNoticeTone(text, .warning);
    }

    pub fn setNoticeTone(self: *HomeView, text: []const u8, tone: NoticeTone) void {
        const length = @min(text.len, self.notice.len);
        @memcpy(self.notice[0..length], text[0..length]);
        self.notice_len = @intCast(length);
        self.notice_tone = tone;
    }

    pub fn render(self: *HomeView, buf: *Buffer, terminal_width: u16, terminal_height: u16, theme: Theme) void {
        if (terminal_width == 0 or terminal_height == 0) return;

        buf.fillRegion(0, 0, terminal_width, terminal_height, .{ .style = .{ .bg = theme.background } });

        const logo_height: u16 = @intCast(LOGO_LINES.len);
        const logo_width: u16 = 39;
        const panel_height: u16 = 4;
        const panel_width = @min(self.prompt_width, terminal_width -| 4);
        const content_height = logo_height + 2 + panel_height + 2;
        const start_y = if (content_height < terminal_height)
            (terminal_height - content_height) / 2
        else
            0;

        const logo_x = if (logo_width < terminal_width) (terminal_width - logo_width) / 2 else 0;
        for (LOGO_LINES, 0..) |line, i| {
            _ = buf.writeStringBounded(logo_x, start_y + @as(u16, @intCast(i)), line, self.logo_style, terminal_width -| logo_x);
        }

        const panel_y = start_y + logo_height + 2;
        if (panel_width > 0 and panel_y < terminal_height) {
            const panel_x = if (panel_width < terminal_width) (terminal_width - panel_width) / 2 else 0;
            const panel_style = Style{ .fg = theme.text, .bg = theme.background_panel };
            buf.fillRegion(panel_x, panel_y, panel_width, @min(panel_height, terminal_height - panel_y), .{ .style = panel_style });

            const rail_color = switch (self.composer_mode) {
                .normal, .multiline => theme.accent,
                .shell => theme.warning,
                .history => theme.info,
                .@"error" => theme.err_color,
            };
            for (0..@min(panel_height, terminal_height - panel_y)) |i| {
                buf.setCell(panel_x, panel_y + @as(u16, @intCast(i)), .{ .char = .{ .char = '▌' }, .style = .{ .fg = rail_color, .bg = theme.background_panel } });
            }

            const content_x = panel_x +| 2;
            const content_width = panel_width -| 3;
            const prompt_y = panel_y +| 1;
            if (content_width > 0 and prompt_y < terminal_height) {
                var text_buf: [512]u8 = undefined;
                const text = self.prompt.gap.getText(&text_buf);
                var first_line_len = text.len;
                for (text, 0..) |byte, i| {
                    if (byte == '\n') {
                        first_line_len = i;
                        break;
                    }
                }
                const first_line = text[0..first_line_len];

                if (first_line.len == 0) {
                    buf.setCell(content_x, prompt_y, .{ .char = .{ .char = '█' }, .style = .{ .fg = theme.prompt_cursor, .bg = theme.background_panel } });
                    _ = buf.writeStringBounded(content_x +| 1, prompt_y, self.placeholder_text, .{ .fg = theme.prompt_placeholder, .bg = theme.background_panel }, content_width -| 1);
                } else {
                    _ = buf.writeStringBounded(content_x, prompt_y, first_line, .{ .fg = theme.text, .bg = theme.background_panel }, content_width);
                    const cursor_x = content_x +| @min(buffer_mod.stringWidth(first_line), content_width -| 1);
                    buf.setCell(cursor_x, prompt_y, .{ .char = .{ .char = '█' }, .style = .{ .fg = theme.prompt_cursor, .bg = theme.background_panel } });
                }
            }

            const metadata = switch (self.composer_mode) {
                .normal => "normal  ·  MiMo-V2.5-Pro  ·  fixture  ·  balanced",
                .multiline => "multiline  ·  MiMo-V2.5-Pro  ·  fixture  ·  balanced",
                .shell => "shell fixture  ·  MiMo-V2.5-Pro  ·  fixture  ·  balanced",
                .history => "history  ·  MiMo-V2.5-Pro  ·  fixture  ·  balanced",
                .@"error" => "error  ·  MiMo-V2.5-Pro  ·  fixture  ·  balanced",
            };
            const metadata_y = panel_y +| 2;
            if (content_width > 0 and metadata_y < terminal_height) {
                _ = buf.writeStringBounded(content_x, metadata_y, metadata, .{ .fg = theme.text_dim, .bg = theme.background_panel }, content_width);
            }

            if (self.notice_len > 0) {
                const notice_y = panel_y +| 3;
                if (content_width > 0 and notice_y < terminal_height) {
                    const notice_color = switch (self.composer_mode) {
                        .shell => theme.warning,
                        .history => theme.info,
                        .@"error" => theme.err_color,
                        .normal, .multiline => switch (self.notice_tone) {
                            .info => theme.info,
                            .success => theme.success,
                            .warning => theme.warning,
                            .@"error" => theme.err_color,
                        },
                    };
                    _ = buf.writeStringBounded(content_x, notice_y, self.notice[0..self.notice_len], .{ .fg = notice_color, .bg = theme.background_panel }, content_width);
                }
            }
        }

        const hint_y = panel_y +| panel_height + 1;
        if (hint_y < terminal_height) {
            const hint = homeHint(self.composer_mode);
            const hint_style = Style{ .fg = theme.text_dim, .bg = theme.background };
            const hint_width = buffer_mod.stringWidth(hint);
            const hint_x = if (hint_width < terminal_width) (terminal_width - hint_width) / 2 else 0;
            _ = buf.writeStringBounded(hint_x, hint_y, hint, hint_style, terminal_width -| hint_x);
        }

        const path_y = terminal_height -| 2;
        const path = "~/omnitrix";
        const version = "v0.1.0-ui-fixture";
        _ = buf.writeStringBounded(1, path_y, path, .{ .fg = theme.text_dim, .bg = theme.background }, terminal_width -| 1);
        const version_width = buffer_mod.stringWidth(version);
        if (version_width + 1 < terminal_width) {
            _ = buf.writeStringBounded(terminal_width - version_width - 1, path_y, version, .{ .fg = theme.text_dim, .bg = theme.background }, version_width);
        }
    }
};
