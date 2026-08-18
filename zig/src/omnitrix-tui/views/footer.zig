const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
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

pub const FooterView = struct {
    info: FooterInfo,

    pub fn init() FooterView {
        return .{ .info = .{} };
    }

    pub fn render(self: FooterView, buf: *Buffer, y: u16, terminal_width: u16, theme: Theme) void {
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
        const mode_text = self.info.mode;
        _ = buf.writeStringBounded(x, y, mode_text, mode_style, @intCast(mode_text.len + 2));
        x += @intCast(mode_text.len + 2);

        // Separator
        buf.setCell(x, y, .{ .char = .{ .char = ' ' }, .style = bg_style });
        x += 1;

        // Model name
        const model_style = Style{ .fg = theme.text, .bg = theme.background_panel };
        _ = buf.writeStringBounded(x, y, self.info.model, model_style, terminal_width -| x);
        x += @as(u16, @intCast(@min(self.info.model.len, terminal_width -| x)));

        // Streaming indicator
        if (self.info.is_streaming) {
            buf.setCell(x + 1, y, .{ .char = .{ .char = '●' }, .style = Style{ .fg = theme.success, .bg = theme.background_panel } });
            x += 3;
        }

        // Thinking indicator
        if (self.info.is_thinking) {
            buf.setCell(x + 1, y, .{ .char = .{ .char = '◆' }, .style = Style{ .fg = theme.warning, .bg = theme.background_panel } });
            x += 3;
        }

        // Push right side to end
        var right_x = terminal_width -| 1;

        // Branch (right-aligned)
        if (self.info.branch.len > 0) {
            const branch_w = @as(u16, @intCast(self.info.branch.len));
            if (right_x > branch_w + 1) {
                right_x -= branch_w + 1;
                const branch_style = Style{ .fg = theme.text_dim, .bg = theme.background_panel };
                _ = buf.writeStringBounded(right_x, y, self.info.branch, branch_style, branch_w);
                // Branch icon
                buf.setCell(right_x - 1, y, .{ .char = .{ .char = '⎇' }, .style = branch_style });
            }
        }

        // Separator
        if (right_x > 2) {
            right_x -= 1;
            buf.setCell(right_x, y, .{ .char = .{ .char = '│' }, .style = Style{ .fg = theme.border, .bg = theme.background_panel } });
        }

        // Token counts
        if (self.info.tokens_prompt > 0 or self.info.tokens_completion > 0) {
            var token_buf: [32]u8 = undefined;
            const token_str = std.fmt.bufPrint(&token_buf, "↑{d} ↓{d}", .{ self.info.tokens_prompt, self.info.tokens_completion }) catch "";
            const token_w = @as(u16, @intCast(token_str.len));
            if (right_x > token_w + 2) {
                right_x -= token_w + 1;
                _ = buf.writeStringBounded(right_x, y, token_str, Style{ .fg = theme.text_dim, .bg = theme.background_panel }, token_w);
            }
        }
    }
};
