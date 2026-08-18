const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const box_mod = @import("../widgets/box.zig");
const text_mod = @import("../widgets/text.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const BoxWidget = box_mod.BoxWidget;
const BorderStyle = box_mod.BorderStyle;
const TextWidget = text_mod.TextWidget;

pub const SidebarInfo = struct {
    session_title: []const u8 = "Untitled Session",
    session_id: []const u8 = "",
    model_name: []const u8 = "MiMo-V2.5-Pro",
    provider_name: []const u8 = "Codebuff",
    branch_name: []const u8 = "masterplan",
    version: []const u8 = "1.18.18",
    workspace_name: ?[]const u8 = null,
    share_url: ?[]const u8 = null,
    files_changed: u32 = 0,
    lines_added: u32 = 0,
    lines_removed: u32 = 0,
};

pub const SidebarView = struct {
    info: SidebarInfo,
    width: u16,

    pub fn init() SidebarView {
        return .{
            .info = .{},
            .width = 38,
        };
    }

    pub fn render(self: SidebarView, buf: *Buffer, rect: Rect, theme: Theme) void {
        // Background
        buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = .{ .bg = theme.background_panel } });

        var y = rect.y + 1;
        const x = rect.x + 2;
        const w = rect.width -| 4;

        // Title
        const title_style = Style{ .fg = theme.text, .bg = theme.background_panel, .attr = .{ .bold = true } };
        _ = buf.writeStringBounded(x, y, self.info.session_title, title_style, w);
        y += 1;

        // Session ID (muted)
        if (self.info.session_id.len > 0) {
            const id_style = Style{ .fg = theme.text_dim, .bg = theme.background_panel };
            _ = buf.writeStringBounded(x, y, self.info.session_id, id_style, w);
            y += 1;
        }

        y += 1; // gap

        // Separator
        const sep_style = Style{ .fg = theme.border, .bg = theme.background_panel };
        for (0..rect.width - 4) |i| {
            buf.setCell(x + @as(u16, @intCast(i)), y, .{ .char = .{ .char = '─' }, .style = sep_style });
        }
        y += 1;

        // Model
        y = self.renderInfoLine(buf, x, y, w, "Model", self.info.model_name, theme);
        y = self.renderInfoLine(buf, x, y, w, "Provider", self.info.provider_name, theme);
        y = self.renderInfoLine(buf, x, y, w, "Branch", self.info.branch_name, theme);

        // Workspace
        if (self.info.workspace_name) |ws| {
            y = self.renderInfoLine(buf, x, y, w, "Workspace", ws, theme);
        }

        y += 1;

        // File changes
        if (self.info.files_changed > 0) {
            const changes_header_style = Style{ .fg = theme.text_muted, .bg = theme.background_panel, .attr = .{ .bold = true } };
            _ = buf.writeStringBounded(x, y, "File Changes", changes_header_style, w);
            y += 1;

            const add_style = Style{ .fg = theme.success, .bg = theme.background_panel };
            const del_style = Style{ .fg = theme.err_color, .bg = theme.background_panel };

            var add_buf: [32]u8 = undefined;
            const add_str = std.fmt.bufPrint(&add_buf, "  +{d}", .{self.info.lines_added}) catch "+0";
            _ = buf.writeStringBounded(x, y, add_str, add_style, w);
            y += 1;

            var del_buf: [32]u8 = undefined;
            const del_str = std.fmt.bufPrint(&del_buf, "  -{d}", .{self.info.lines_removed}) catch "-0";
            _ = buf.writeStringBounded(x, y, del_str, del_style, w);
            y += 1;
        }

        // Share URL
        if (self.info.share_url) |url| {
            y += 1;
            const url_style = Style{ .fg = theme.accent, .bg = theme.background_panel };
            _ = buf.writeStringBounded(x, y, "Share:", Style{ .fg = theme.text_muted, .bg = theme.background_panel }, w);
            y += 1;
            _ = buf.writeStringBounded(x + 1, y, url, url_style, w -| 1);
            y += 1;
        }

        // Push to bottom
        y = rect.y + rect.height -| 2;

        // Version footer
        const version_style = Style{ .fg = theme.text_muted, .bg = theme.background_panel };
        const version_text = self.info.version;
        _ = buf.writeStringBounded(x, y, "Open", Style{ .fg = theme.success, .bg = theme.background_panel, .attr = .{ .bold = true } }, w);
        y += 1;
        _ = buf.writeStringBounded(x, y, version_text, version_style, w);
    }

    fn renderInfoLine(self: SidebarView, buf: *Buffer, x: u16, y: u16, w: u16, label: []const u8, value: []const u8, theme: Theme) u16 {
        _ = self;
        const label_style = Style{ .fg = theme.text_muted, .bg = theme.background_panel };
        const value_style = Style{ .fg = theme.text, .bg = theme.background_panel };
        _ = buf.writeStringBounded(x, y, label, label_style, w);
        _ = buf.writeStringBounded(x + 1, y + 1, value, value_style, w -| 1);
        return y + 2;
    }
};

const Rect = @import("../core/layout.zig").Rect;
