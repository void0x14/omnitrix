//! omnitrix-tui: OpenCode Stili Kart Tabanlı Konuşma Bloğu Renderlayıcı (Bölüm 5.1, Kol A - A2)
//!
//! Özellikler:
//! - Rounded kart kutuları (User, Agent, Tool, System).
//! - Açılabilir/katlanabilir Tool blokları (araç parametreleri ve çıktıları).
//! - Canlı safe-tail streaming token renderer.
//! - Renk paleti ve ikonlar (OpenCode & Grok stili).

const std = @import("std");
const unicode = @import("unicode.zig");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const Color = term.Color;

pub const BlockKind = enum {
    user,
    agent,
    tool,
    system,

    pub fn label(self: BlockKind) []const u8 {
        return @tagName(self);
    }
};

pub const BlockStatus = enum {
    in_progress,
    completed,
    failed,
};

/// Tek bir konuşma bloğu
pub const Block = struct {
    id: u64,
    kind: BlockKind,
    header: []const u8,
    content: std.ArrayList(u8),
    status: BlockStatus = .completed,
    tool_collapsed: bool = true,
    tool_name: ?[]const u8 = null,
    tool_summary: ?[]const u8 = null,
    duration_ms: ?u64 = null,
    timestamp_ns: i128 = 0,
    revision: u64 = 0,

    pub fn init(
        allocator: std.mem.Allocator,
        id: u64,
        kind: BlockKind,
        header: []const u8,
        initial_content: []const u8,
    ) !Block {
        var content = std.ArrayList(u8).empty;
        errdefer content.deinit(allocator);
        try content.appendSlice(allocator, initial_content);

        return .{
            .id = id,
            .kind = kind,
            .header = try allocator.dupe(u8, header),
            .content = content,
            .status = .completed,
            .tool_collapsed = (kind == .tool),
            .tool_name = null,
            .tool_summary = null,
            .duration_ms = null,
            .timestamp_ns = 0,
            .revision = 0,
        };
    }

    pub fn deinit(self: *Block, allocator: std.mem.Allocator) void {
        allocator.free(self.header);
        self.content.deinit(allocator);
        if (self.tool_name) |tn| allocator.free(tn);
        if (self.tool_summary) |ts| allocator.free(ts);
        self.* = undefined;
    }

    pub fn toggleCollapse(self: *Block) void {
        if (self.kind == .tool) {
            self.tool_collapsed = !self.tool_collapsed;
        }
    }
};

/// Bounded Conversation Block Yöneticisi ve Renderer
pub const BlockRenderer = struct {
    allocator: std.mem.Allocator,
    blocks: std.ArrayList(Block),
    max_blocks: usize,
    next_block_id: u64 = 1,
    active_block_id: ?u64 = null,
    last_rendered_rev: u64 = 0,

    pub fn init(allocator: std.mem.Allocator, max_blocks: usize) BlockRenderer {
        return .{
            .allocator = allocator,
            .blocks = std.ArrayList(Block).empty,
            .max_blocks = max_blocks,
            .next_block_id = 1,
            .active_block_id = null,
            .last_rendered_rev = 0,
        };
    }

    pub fn deinit(self: *BlockRenderer) void {
        for (self.blocks.items) |*b| {
            b.deinit(self.allocator);
        }
        self.blocks.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addBlock(
        self: *BlockRenderer,
        kind: BlockKind,
        header: []const u8,
        content: []const u8,
    ) !u64 {
        if (self.blocks.items.len >= self.max_blocks and self.max_blocks > 0) {
            var old = self.blocks.orderedRemove(0);
            old.deinit(self.allocator);
        }

        const id = self.next_block_id;
        self.next_block_id += 1;

        var block = try Block.init(self.allocator, id, kind, header, content);
        errdefer block.deinit(self.allocator);

        try self.blocks.append(self.allocator, block);
        self.active_block_id = id;
        return id;
    }

    pub fn addToolBlock(
        self: *BlockRenderer,
        tool_name: []const u8,
        summary: []const u8,
        full_output: []const u8,
    ) !u64 {
        const id = try self.addBlock(.tool, tool_name, full_output);
        if (self.getBlock(id)) |b| {
            b.tool_name = try self.allocator.dupe(u8, tool_name);
            b.tool_summary = try self.allocator.dupe(u8, summary);
            b.tool_collapsed = true;
            b.duration_ms = 12;
        }
        return id;
    }

    pub fn getBlock(self: *BlockRenderer, id: u64) ?*Block {
        for (self.blocks.items) |*b| {
            if (b.id == id) return b;
        }
        return null;
    }

    pub fn toggleToolCollapse(self: *BlockRenderer, id: u64) bool {
        if (self.getBlock(id)) |b| {
            b.toggleCollapse();
            return true;
        }
        return false;
    }

    pub fn appendStreamingChunk(self: *BlockRenderer, block_id: u64, chunk: []const u8, revision: u64) !void {
        const block = self.getBlock(block_id) orelse return error.BlockNotFound;
        try block.content.appendSlice(self.allocator, chunk);
        block.revision = revision;
        block.status = .in_progress;
    }

    pub fn finishBlock(self: *BlockRenderer, block_id: u64, status: BlockStatus) void {
        if (self.getBlock(block_id)) |b| {
            b.status = status;
        }
    }

    /// Blok listesini OpenCode kartları olarak render eder.
    pub fn renderToLines(
        self: *const BlockRenderer,
        allocator: std.mem.Allocator,
        max_width: usize,
    ) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        if (max_width < 20) return lines;
        const inner_w = if (max_width > 4) max_width - 4 else 16;

        for (self.blocks.items) |block| {
            const border_col = switch (block.kind) {
                .user => Color{ .ansi = 75 },
                .agent => Color{ .ansi = 114 },
                .tool => Color{ .ansi = 179 },
                .system => Color{ .ansi = 243 },
            };

            const icon_str = switch (block.kind) {
                .user => " 👤 ",
                .agent => " 🤖 ",
                .tool => " 🛠️ ",
                .system => " ⚙️ ",
            };

            // 1. Kart Başlığı: ╭─ 🤖 Omnitrix ─────────────── [✓] ─╮
            var top_buf = std.ArrayList(u8).empty;
            defer top_buf.deinit(allocator);

            try term.appendStyle(&top_buf, allocator, .{ .fg = border_col });
            try top_buf.appendSlice(allocator, theme.Box.top_left);
            try top_buf.appendSlice(allocator, theme.Box.horizontal);

            // İkon ve İsim
            try term.appendStyle(&top_buf, allocator, .{ .fg = border_col, .bold = true });
            try top_buf.appendSlice(allocator, icon_str);
            try top_buf.appendSlice(allocator, block.header);
            try top_buf.appendSlice(allocator, " ");
            try term.appendStyle(&top_buf, allocator, .{ .fg = border_col });

            // Durum rozeti (sağ taraf)
            var badge_buf: [64]u8 = undefined;
            const badge_str = switch (block.status) {
                .in_progress => " [⏳ running] ",
                .completed => if (block.duration_ms) |d|
                    try std.fmt.bufPrint(&badge_buf, " [✓ done {d}ms] ", .{d})
                else
                    " [✓ done] ",
                .failed => " [✗ failed] ",
            };

            const header_w = unicode.strWidth(icon_str) + unicode.strWidth(block.header) + 3;
            const badge_w = unicode.strWidth(badge_str);

            var t_pad = if (max_width > header_w + badge_w + 2) max_width - header_w - badge_w - 2 else 1;
            while (t_pad > 0) : (t_pad -= 1) {
                try top_buf.appendSlice(allocator, theme.Box.horizontal);
            }

            try term.appendStyle(&top_buf, allocator, .{ .fg = theme.Theme.text_dim });
            try top_buf.appendSlice(allocator, badge_str);
            try term.appendStyle(&top_buf, allocator, .{ .fg = border_col });
            try top_buf.appendSlice(allocator, theme.Box.top_right);
            try top_buf.appendSlice(allocator, theme.ANSI.reset);

            try lines.append(allocator, try top_buf.toOwnedSlice(allocator));

            // Katlanmış tool ise sadece özet satırını bas
            if (block.kind == .tool and block.tool_collapsed) {
                var tool_sum_buf = std.ArrayList(u8).empty;
                defer tool_sum_buf.deinit(allocator);

                try term.appendStyle(&tool_sum_buf, allocator, .{ .fg = border_col });
                try tool_sum_buf.appendSlice(allocator, theme.Box.vertical);
                try tool_sum_buf.appendSlice(allocator, "  ");

                try term.appendStyle(&tool_sum_buf, allocator, .{ .fg = theme.Theme.text_dim, .italic = true });
                const sum_text = block.tool_summary orelse "(Click or press 't' to expand output)";
                const trunc_sum = try unicode.truncateToWidth(allocator, sum_text, inner_w - 4, "...");
                defer allocator.free(trunc_sum);
                try tool_sum_buf.appendSlice(allocator, trunc_sum);

                const sum_w = unicode.strWidth(trunc_sum) + 4;
                var s_pad = if (max_width > sum_w + 1) max_width - sum_w - 1 else 1;
                try term.appendStyle(&tool_sum_buf, allocator, .{ .fg = border_col });
                while (s_pad > 0) : (s_pad -= 1) {
                    try tool_sum_buf.appendSlice(allocator, " ");
                }
                try tool_sum_buf.appendSlice(allocator, theme.Box.vertical);
                try tool_sum_buf.appendSlice(allocator, theme.ANSI.reset);
                try lines.append(allocator, try tool_sum_buf.toOwnedSlice(allocator));
            } else {
                // Kart İçerik Satırları
                var content_lines = try unicode.wrapText(allocator, block.content.items, inner_w);
                defer {
                    for (content_lines.items) |cl| allocator.free(cl);
                    content_lines.deinit(allocator);
                }

                if (content_lines.items.len == 0) {
                    var empty_buf = std.ArrayList(u8).empty;
                    defer empty_buf.deinit(allocator);
                    try term.appendStyle(&empty_buf, allocator, .{ .fg = border_col });
                    try empty_buf.appendSlice(allocator, theme.Box.vertical);
                    var ep = if (max_width > 2) max_width - 2 else 1;
                    while (ep > 0) : (ep -= 1) {
                        try empty_buf.appendSlice(allocator, " ");
                    }
                    try empty_buf.appendSlice(allocator, theme.Box.vertical);
                    try empty_buf.appendSlice(allocator, theme.ANSI.reset);
                    try lines.append(allocator, try empty_buf.toOwnedSlice(allocator));
                }

                for (content_lines.items) |cl| {
                    var line_buf = std.ArrayList(u8).empty;
                    defer line_buf.deinit(allocator);

                    try term.appendStyle(&line_buf, allocator, .{ .fg = border_col });
                    try line_buf.appendSlice(allocator, theme.Box.vertical);
                    try line_buf.appendSlice(allocator, " ");

                    try term.appendStyle(&line_buf, allocator, .{ .fg = theme.Theme.text_main });
                    try line_buf.appendSlice(allocator, cl);

                    const line_w = unicode.strWidth(cl) + 3;
                    var l_pad = if (max_width > line_w + 1) max_width - line_w - 1 else 1;
                    try term.appendStyle(&line_buf, allocator, .{ .fg = border_col });
                    while (l_pad > 0) : (l_pad -= 1) {
                        try line_buf.appendSlice(allocator, " ");
                    }
                    try line_buf.appendSlice(allocator, theme.Box.vertical);
                    try line_buf.appendSlice(allocator, theme.ANSI.reset);

                    try lines.append(allocator, try line_buf.toOwnedSlice(allocator));
                }
            }

            // 3. Alt Kenarlık: ╰────────────────────────────────────────────╯
            var bot_buf = std.ArrayList(u8).empty;
            defer bot_buf.deinit(allocator);

            try term.appendStyle(&bot_buf, allocator, .{ .fg = border_col });
            try bot_buf.appendSlice(allocator, theme.Box.bottom_left);
            var b_pad = if (max_width > 2) max_width - 2 else 1;
            while (b_pad > 0) : (b_pad -= 1) {
                try bot_buf.appendSlice(allocator, theme.Box.horizontal);
            }
            try bot_buf.appendSlice(allocator, theme.Box.bottom_right);
            try bot_buf.appendSlice(allocator, theme.ANSI.reset);

            try lines.append(allocator, try bot_buf.toOwnedSlice(allocator));
            try lines.append(allocator, try allocator.dupe(u8, "")); // Kartlar arası nefes payı
        }

        return lines;
    }
};

test "block renderer rounded card cizimi ve tool collapse/expand" {
    var renderer = BlockRenderer.init(std.testing.allocator, 10);
    defer renderer.deinit();

    _ = try renderer.addBlock(.user, "User", "Inspect src/main.zig");
    const tool_id = try renderer.addToolBlock("edit", "path: src/main.zig", "pub fn main() void {\n    // fixed\n}");

    var lines_col = try renderer.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines_col.items) |l| std.testing.allocator.free(l);
        lines_col.deinit(std.testing.allocator);
    }
    try std.testing.expect(lines_col.items.len > 0);

    // Expand
    try std.testing.expect(renderer.toggleToolCollapse(tool_id));
    var lines_exp = try renderer.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines_exp.items) |l| std.testing.allocator.free(l);
        lines_exp.deinit(std.testing.allocator);
    }
    try std.testing.expect(lines_exp.items.len > lines_col.items.len);
}
