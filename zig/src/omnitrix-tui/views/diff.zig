const std = @import("std");
const changes_mod = @import("changes.zig");
const sidebar_mod = @import("sidebar.zig");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const layout_mod = @import("../core/layout.zig");
const theme_mod = @import("../core/theme.zig");

const Buffer = buffer_mod.Buffer;
const Rect = layout_mod.Rect;
const Style = cell_mod.Style;
const Theme = theme_mod.Theme;
const SidebarInfo = sidebar_mod.SidebarInfo;

pub const DiffState = struct {
    selected_file: ?u16 = null,
    selected_hunk: ?u16 = null,
    fullscreen: bool = false,
};

pub fn render(buf: *Buffer, rect: Rect, info: SidebarInfo, state: DiffState, theme: Theme) void {
    buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = .{ .bg = theme.background } });
    if (rect.width < 12 or rect.height == 0) return;

    const count = @min(info.changed_files.len, changes_mod.max_rows);
    if (count == 0 or state.selected_file == null) {
        _ = buf.writeStringBounded(rect.x + 1, rect.y, "DIFF", .{ .fg = theme.text, .bg = theme.background, .attr = .{ .bold = true } }, rect.width -| 2);
        _ = buf.writeStringBounded(rect.x + 2, rect.y +| 2, "Diff unavailable: no changed file is selected.", .{ .fg = theme.warning, .bg = theme.background }, rect.width -| 4);
        return;
    }

    const selected = @min(@as(usize, @intCast(state.selected_file.?)), count - 1);
    const file = info.changed_files[selected];
    const title_style = Style{ .fg = theme.text, .bg = theme.background, .attr = .{ .bold = true } };
    var title_buf: [128]u8 = undefined;
    const title = std.fmt.bufPrint(&title_buf, "DIFF{s}  {s}", .{ if (state.fullscreen) " FULLSCREEN" else "", file.path }) catch "DIFF";
    _ = buf.writeStringBounded(rect.x + 1, rect.y, title, title_style, rect.width -| 2);

    var meta_buf: [96]u8 = undefined;
    const meta = std.fmt.bufPrint(&meta_buf, "File {d}/{d}  •  hunk {d}/1  •  fixture preview", .{ selected + 1, count, @as(usize, @intCast(state.selected_hunk orelse 0)) + 1 }) catch "fixture preview";
    _ = buf.writeStringBounded(rect.x + 1, rect.y +| 1, meta, .{ .fg = theme.text_dim, .bg = theme.background }, rect.width -| 2);


    var add_buf: [96]u8 = undefined;
    const add_line = std.fmt.bufPrint(&add_buf, "+ fixture additions: {d}", .{file.added}) catch "+ fixture additions";
    _ = buf.writeStringBounded(rect.x + 2, rect.y +| 3, add_line, .{ .fg = theme.diff_add_fg, .bg = theme.diff_add_bg }, rect.width -| 4);
    var del_buf: [96]u8 = undefined;
    const del_line = std.fmt.bufPrint(&del_buf, "- fixture deletions: {d}", .{file.removed}) catch "- fixture deletions";
    _ = buf.writeStringBounded(rect.x + 2, rect.y +| 4, del_line, .{ .fg = theme.diff_del_fg, .bg = theme.diff_del_bg }, rect.width -| 4);
}
