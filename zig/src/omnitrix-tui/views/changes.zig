const std = @import("std");
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

pub const ChangeStatus = enum { added, modified, deleted, renamed, binary, conflict, unavailable };

pub const ChangeRow = struct {
    status: ChangeStatus,
    path: []const u8,
    additions: u32 = 0,
    deletions: u32 = 0,
    actor: []const u8 = "",
};

pub const max_rows: usize = 6;

fn row(info: SidebarInfo, index: usize) ChangeRow {
    const file = info.changed_files[index];
    return .{
        .status = .modified,
        .path = file.path,
        .additions = file.added,
        .deletions = file.removed,
        .actor = "fixture",
    };
}

fn statusText(status: ChangeStatus) []const u8 {
    return switch (status) {
        .added => "A",
        .modified => "M",
        .deleted => "D",
        .renamed => "R",
        .binary => "B",
        .conflict => "!",
        .unavailable => "?",
    };
}

fn statusColor(status: ChangeStatus, theme: Theme) @TypeOf(theme.text) {
    return switch (status) {
        .added => theme.diff_add_fg,
        .deleted => theme.diff_del_fg,
        .conflict => theme.warning,
        .unavailable => theme.text_dim,
        else => theme.accent,
    };
}

pub fn render(buf: *Buffer, rect: Rect, info: SidebarInfo, selected_row: ?u16, theme: Theme) void {
    buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = .{ .bg = theme.background } });
    if (rect.width < 8 or rect.height == 0) return;

    const count = @min(info.changed_files.len, max_rows);
    const title_style = Style{ .fg = theme.text, .bg = theme.background, .attr = .{ .bold = true } };
    var title_buf: [64]u8 = undefined;
    const title = std.fmt.bufPrint(&title_buf, "CHANGES  {d} files", .{count}) catch "CHANGES";
    _ = buf.writeStringBounded(rect.x + 1, rect.y, title, title_style, rect.width -| 2);

    if (count == 0) {
        _ = buf.writeStringBounded(rect.x + 2, rect.y +| 2, "No changed files in fixture.", .{ .fg = theme.text_muted, .bg = theme.background }, rect.width -| 4);
        return;
    }

    const header_y = rect.y +| 2;
    _ = buf.writeStringBounded(rect.x + 1, header_y, "ST PATH                                      + / -   ACTOR", .{ .fg = theme.text_dim, .bg = theme.background }, rect.width -| 2);

    for (0..count) |i| {
        const row_y = header_y +| 1 +| @as(u16, @intCast(i));
        if (row_y >= rect.y +| rect.height) break;
        const change = row(info, i);
        const selected = selected_row != null and selected_row.? == @as(u16, @intCast(i));
        const bg = if (selected) theme.background_elevated else theme.background;
        const path_style = Style{ .fg = if (selected) theme.text else theme.text_muted, .bg = bg };
        const marker_style = Style{ .fg = statusColor(change.status, theme), .bg = bg, .attr = .{ .bold = true } };
        buf.fillRegion(rect.x + 1, row_y, rect.width -| 2, 1, .{ .style = .{ .bg = bg } });
        _ = buf.writeStringBounded(rect.x + 1, row_y, statusText(change.status), marker_style, 1);
        _ = buf.writeStringBounded(rect.x + 3, row_y, change.path, path_style, rect.width -| 24);

        var count_buf: [32]u8 = undefined;
        const counts = std.fmt.bufPrint(&count_buf, "+{d} -{d}", .{ change.additions, change.deletions }) catch "+0 -0";
        const counts_x = rect.x + rect.width -| 20;
        _ = buf.writeStringBounded(counts_x, row_y, counts, .{ .fg = theme.text_muted, .bg = bg }, rect.width -| (counts_x - rect.x));
        _ = buf.writeStringBounded(rect.x + rect.width -| 9, row_y, change.actor, .{ .fg = theme.accent_muted, .bg = bg }, 8);
    }
}
