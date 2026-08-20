const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const ui_state_mod = @import("../core/ui_state.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;

pub const FooterInfo = struct {
    mode: []const u8 = "build",
    model: []const u8 = "MiMo-V2.5-Pro",
    tokens_prompt: u32 = 0,
    tokens_completion: u32 = 0,
    is_streaming: bool = false,
    is_thinking: bool = false,
    session_time: u64 = 0,
    branch: []const u8 = "masterplan",
    focused_panel: []const u8 = "conversation",
};

pub const FooterContext = struct {
    route: ui_state_mod.Route = .home,
    focus: ui_state_mod.FocusTarget = .prompt,
    auth_state: ?u8 = null,
    composer_mode: ?[]const u8 = null,
    info: ?FooterInfo = null,
};

pub const FooterView = struct {
    info: FooterInfo,

    pub fn init() FooterView {
        return .{ .info = .{} };
    }

    pub fn renderContext(self: FooterView, buf: *Buffer, y: u16, terminal_width: u16, theme: Theme, context: FooterContext) void {
        switch (context.route) {
            .session => self.renderSessionFooter(buf, y, terminal_width, theme, context),
            else => {
                const style = Style{ .fg = theme.text_dim, .bg = theme.background_panel };
                for (0..terminal_width) |i| buf.setCell(@intCast(i), y, .{ .style = style });
                const hint = contextualHint(context);
                const hint_width = buffer_mod.stringWidth(hint);
                const x = if (hint_width < terminal_width) (terminal_width - hint_width) / 2 else 0;
                _ = buf.writeStringBounded(x, y, hint, style, terminal_width -| x);
            },
        }
    }

    pub fn render(self: FooterView, buf: *Buffer, y: u16, terminal_width: u16, theme: Theme) void {
        self.renderContext(buf, y, terminal_width, theme, .{
            .route = .session,
            .focus = focusForPanel(self.info.focused_panel),
            .info = self.info,
        });
    }

    fn renderSessionFooter(self: FooterView, buf: *Buffer, y: u16, terminal_width: u16, theme: Theme, context: FooterContext) void {
        const info = context.info orelse self.info;
        // Background
        const bg_style = Style{ .fg = theme.text_muted, .bg = theme.background_panel };
        for (0..terminal_width) |i| {
            buf.setCell(@as(u16, @intCast(i)), y, .{ .style = bg_style });
        }

        var x: u16 = 1;

        // Mode indicator
        const mode_style = Style{
            .fg = theme.background_panel,
            .bg = theme.accent,
            .attr = .{ .bold = true },
        };
        const mode_text = info.mode;
        _ = buf.writeStringBounded(x, y, mode_text, mode_style, @intCast(mode_text.len + 2));
        x += @intCast(mode_text.len + 2);

        // Separator
        buf.setCell(x, y, .{ .char = .{ .char = ' ' }, .style = bg_style });
        x += 1;

        // Model name
        const model_style = Style{ .fg = theme.text, .bg = theme.background_panel };
        _ = buf.writeStringBounded(x, y, info.model, model_style, terminal_width -| x);
        x += @as(u16, @intCast(@min(info.model.len, terminal_width -| x)));

        // Streaming indicator
        if (info.is_streaming) {
            buf.setCell(x + 1, y, .{ .char = .{ .char = '●' }, .style = Style{ .fg = theme.success, .bg = theme.background_panel } });
            x += 3;
        }

        // Thinking indicator
        if (info.is_thinking) {
            buf.setCell(x + 1, y, .{ .char = .{ .char = '◆' }, .style = Style{ .fg = theme.warning, .bg = theme.background_panel } });
            x += 3;
        }

        // Push right side to end
        var right_x = terminal_width -| 1;

        // The existing SessionView call has no FooterContext parameter. Its
        // focused panel remains the truthful fallback context for this path.
        const panel_label = if (std.mem.eql(u8, info.focused_panel, "changes"))
            "changes"
        else if (std.mem.eql(u8, info.focused_panel, "diff"))
            "diff"
        else
            "conversation";
        const panel_width: u16 = @intCast(panel_label.len);
        if (right_x > panel_width + 2) {
            right_x -= panel_width;
            _ = buf.writeStringBounded(right_x, y, panel_label, Style{ .fg = theme.text_dim, .bg = theme.background_panel }, panel_width);
            right_x -|= 1;
        }

        // Branch (right-aligned, clean)
        if (info.branch.len > 0) {
            const branch_w = @as(u16, @intCast(info.branch.len));
            if (right_x > branch_w + 1) {
                right_x -= branch_w;
                const branch_style = Style{ .fg = theme.text_dim, .bg = theme.background_panel };
                _ = buf.writeStringBounded(right_x, y, info.branch, branch_style, branch_w);
            }
        }

        // Separator
        if (right_x > 2) {
            right_x -= 1;
            buf.setCell(right_x, y, .{ .char = .{ .char = '│' }, .style = Style{ .fg = theme.border, .bg = theme.background_panel } });
        }

        // Token counts (clean format without arrows)
        if (info.tokens_prompt > 0 or info.tokens_completion > 0) {
            var token_buf: [32]u8 = undefined;
            const token_str = std.fmt.bufPrint(&token_buf, "{d} tok", .{info.tokens_prompt + info.tokens_completion}) catch "";
            const token_w = @as(u16, @intCast(token_str.len));
            if (right_x > token_w + 2) {
                right_x -= token_w + 1;
                _ = buf.writeStringBounded(right_x, y, token_str, Style{ .fg = theme.text_dim, .bg = theme.background_panel }, token_w);
            }
        }
    }
};

pub fn renderWelcomeContext(buf: *Buffer, y: u16, terminal_width: u16, theme: Theme, auth_state: u8) void {
    var footer = FooterView.init();
    footer.renderContext(buf, y, terminal_width, theme, .{ .route = .welcome, .auth_state = auth_state });
}

fn focusForPanel(panel: []const u8) ui_state_mod.FocusTarget {
    if (std.mem.eql(u8, panel, "changes") or std.mem.eql(u8, panel, "diff")) return .sidebar;
    if (std.mem.eql(u8, panel, "prompt")) return .prompt;
    return .conversation;
}

fn contextualHint(context: FooterContext) []const u8 {
    return switch (context.route) {
        .welcome => if (context.auth_state) |state| switch (state) {
            0 => "Enter choose method   1/2/3 select fixture",
            1 => "↑↓ choose method   Enter continue   Esc cancel",
            2 => "Enter complete fixture   F fail fixture   Esc cancel",
            3 => "Opening home",
            4 => "R retry   Enter retry   Esc cancel",
            else => "Enter choose method",
        } else "Enter choose method",
        .home => if (context.composer_mode) |mode| if (std.mem.eql(u8, mode, "shell"))
            "Tab normal   Enter shell fixture   Esc cancel"
        else if (std.mem.eql(u8, mode, "multiline"))
            "Enter send   Shift+Enter newline   Tab normal   Esc cancel"
        else if (std.mem.eql(u8, mode, "history"))
            "Up/Down history fixture   Esc close"
        else if (std.mem.eql(u8, mode, "error"))
            "Esc dismiss   Ctrl+K clear   edit prompt"
        else
            "Enter send   Shift+Enter multiline   Tab shell   Ctrl+R history"
        else "Enter send   Shift+Enter multiline   Tab shell",
        .session => switch (context.focus) {
            .prompt => "Enter send   Tab mode   Shift+Tab tabs   Ctrl+P commands",
            .conversation => "↑↓ scroll   Shift+Tab tabs   Enter focus prompt",
            .sidebar => "←→ tabs   Shift+Tab tabs   Ctrl+B sidebar",
            .overlay => "Esc close overlay   Enter confirm",
            .page => "Tab focus   Ctrl+P commands",
        },
        .too_small => "Resize the terminal to continue",
    };
}
