const std = @import("std");
const buffer_mod = @import("../core/buffer.zig");
const cell_mod = @import("../core/cell.zig");
const layout_mod = @import("../core/layout.zig");
const theme_mod = @import("../core/theme.zig");
const markdown_mod = @import("../widgets/markdown.zig");
const scroll_mod = @import("../widgets/scroll.zig");
const spinner_mod = @import("../widgets/spinner.zig");

const Buffer = buffer_mod.Buffer;
const Style = cell_mod.Style;
const Rect = layout_mod.Rect;
const Theme = theme_mod.Theme;
const MarkdownRenderer = markdown_mod.MarkdownRenderer;
const ScrollContainer = scroll_mod.ScrollContainer;
const Spinner = spinner_mod.Spinner;

pub const BlockKind = enum {
    text,
    thinking,
    tool_use,
    tool_result,
    code,
    @"error",
};

pub const Block = struct {
    id: u32,
    kind: BlockKind,
    text: []const u8,
    committed_len: usize,
    collapsed: bool = false,
    is_streaming: bool = false,
};

pub const BlockStore = struct {
    arena: std.heap.ArenaAllocator,
    blocks: std.ArrayList(Block),
    max_blocks: usize = 512,

    pub fn init(parent: std.mem.Allocator) BlockStore {
        return .{
            .arena = std.heap.ArenaAllocator.init(parent),
            .blocks = .empty,
        };
    }

    pub fn deinit(self: *BlockStore) void {
        self.blocks.deinit(self.arena.allocator());
        self.arena.deinit();
    }

    pub fn append(self: *BlockStore, block: Block) !void {
        if (self.max_blocks == 0) return;
        const owned_text = try self.arena.allocator().dupe(u8, block.text);
        var owned = block;
        owned.text = owned_text;
        owned.committed_len = @min(block.committed_len, owned_text.len);
        if (self.blocks.items.len == self.max_blocks) {
            _ = self.blocks.orderedRemove(0);
        }
        try self.blocks.append(self.arena.allocator(), owned);
    }

    pub fn toggleCollapsed(self: *BlockStore, id: u32) bool {
        for (self.blocks.items) |*block| {
            if (block.id == id) {
                block.collapsed = !block.collapsed;
                return true;
            }
        }
        return false;
    }

    pub fn safeText(block: Block) []const u8 {
        if (!block.is_streaming) return block.text;
        return block.text[0..@min(block.committed_len, block.text.len)];
    }
};

const max_tool_detail: usize = 1024;
const max_summary: usize = 112;

pub fn render(
    store: *const BlockStore,
    markdown: MarkdownRenderer,
    spinner: *Spinner,
    scroll: *ScrollContainer,
    buf: *Buffer,
    rect: Rect,
    theme: Theme,
) void {
    if (rect.width == 0 or rect.height == 0) return;
    buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = .{ .bg = theme.background } });

    if (store.blocks.items.len == 0) {
        _ = buf.writeStringBounded(rect.x + 2, rect.y + rect.height / 2, "No messages yet. Start typing below...", theme.dimStyle(), rect.width -| 4);
        scroll.setContentHeight(0);
        return;
    }

    const inner_width = rect.width -| 4;
    var content_y: u16 = 0;
    for (store.blocks.items) |block| {
        const height = blockHeight(markdown, block, inner_width);
        if (scroll.contentToScreen(content_y)) |screen_y| {
            if (screen_y < rect.height) {
                renderBlock(markdown, block, buf, rect.x + 2, rect.y + screen_y, inner_width, theme);
            }
        }
        content_y +|= height;
    }

    if (store.blocks.items[store.blocks.items.len - 1].is_streaming) {
        const status_y = content_y;
        if (scroll.contentToScreen(status_y)) |screen_y| {
            if (screen_y < rect.height) {
                spinner.tick();
                spinner.render(buf, rect.x + 2, rect.y + screen_y, inner_width);
            }
        }
        content_y +|= 1;
    }

    scroll.setContentHeight(content_y);
    scroll.renderScrollbar(buf, rect.x + rect.width - 1, rect.y, rect.height);
}

pub fn blockAtScreenY(store: *const BlockStore, scroll: ScrollContainer, rect: Rect, screen_y: u16) ?u32 {
    if (screen_y < rect.y or screen_y >= rect.y + rect.height) return null;
    const content_y = scroll.screenToContent(screen_y - rect.y);
    var row: u16 = 0;
    for (store.blocks.items) |block| {
        const height = blockHeightSimple(block, rect.width -| 4);
        if (content_y >= row and content_y < row + height) return block.id;
        row +|= height;
    }
    return null;
}

fn blockHeight(markdown: MarkdownRenderer, block: Block, width: u16) u16 {
    if (block.kind == .tool_use or block.kind == .tool_result) {
        return if (block.collapsed) 1 else 1 + markdown.measureHeight(width, boundedText(BlockStore.safeText(block), max_tool_detail));
    }
    if (block.kind == .code) return 1 + codeLineCount(BlockStore.safeText(block), width);
    return 1 + markdown.measureHeight(width, BlockStore.safeText(block));
}

fn blockHeightSimple(block: Block, width: u16) u16 {
    const text = BlockStore.safeText(block);
    if (block.kind == .tool_use or block.kind == .tool_result) {
        return if (block.collapsed) 1 else 1 + boundedLineCount(text, width, max_tool_detail);
    }
    if (block.kind == .code) return 1 + codeLineCount(text, width);
    return 1 + boundedLineCount(text, width, text.len);
}

fn renderBlock(markdown: MarkdownRenderer, block: Block, buf: *Buffer, x: u16, y: u16, width: u16, theme: Theme) void {
    const text = BlockStore.safeText(block);
    var label_buf: [96]u8 = undefined;
    const label = std.fmt.bufPrint(&label_buf, "{s} ", .{kindLabel(block.kind)}) catch kindLabel(block.kind);
    const label_style = switch (block.kind) {
        .@"error" => Style{ .fg = theme.err_color, .bg = theme.background, .attr = .{ .bold = true } },
        .thinking => Style{ .fg = theme.text_dim, .bg = theme.background, .attr = .{ .italic = true } },
        .tool_use => Style{ .fg = theme.info, .bg = theme.background, .attr = .{ .bold = true } },
        .tool_result => Style{ .fg = theme.success_muted, .bg = theme.background, .attr = .{ .bold = true } },
        .code => Style{ .fg = theme.syntax_keyword, .bg = theme.background, .attr = .{ .bold = true } },
        .text => theme.mutedStyle(),
    };
    const marker: []const u8 = if ((block.kind == .tool_use or block.kind == .tool_result) and !block.collapsed) "▼" else if (block.kind == .tool_use or block.kind == .tool_result) "▶" else "";
    var prefix_buf: [8]u8 = undefined;
    const prefix = std.fmt.bufPrint(&prefix_buf, "{s}{s}", .{ marker, label }) catch label;
    _ = buf.writeStringBounded(x, y, prefix, label_style, width);

    const detail_x = x +| @min(buffer_mod.stringWidth(prefix) + 1, width);
    const detail_width = width -| (detail_x - x);
    if (detail_width == 0) return;

    switch (block.kind) {
        .tool_use, .tool_result => {
            if (block.collapsed) {
                _ = buf.writeStringBounded(detail_x, y, boundedText(text, max_summary), theme.mutedStyle(), detail_width);
            } else {
                _ = markdown.render(buf, detail_x, y + 1, detail_width, boundedText(text, max_tool_detail));
            }
        },
        .thinking => {
            _ = markdown.render(buf, detail_x, y, detail_width, boundedText(text, max_tool_detail));
        },
        .@"error" => {
            _ = markdown.render(buf, detail_x, y, detail_width, boundedText(text, max_tool_detail));
        },
        .code => renderCode(buf, detail_x, y + 1, detail_width, boundedText(text, max_tool_detail), theme),
        .text => _ = markdown.render(buf, detail_x, y, detail_width, text),
    }
}

fn renderCode(buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8, theme: Theme) void {
    var row = y;
    var lines = std.mem.splitScalar(u8, text, '\n');
    const style = Style{ .fg = theme.syntax_string, .bg = theme.background_panel };
    while (lines.next()) |line| {
        if (row >= buf.rows) break;
        buf.fillRegion(x, row, width, 1, .{ .style = style });
        _ = buf.writeStringBounded(x + 1, row, line, style, width -| 1);
        row += 1;
    }
}

fn kindLabel(kind: BlockKind) []const u8 {
    return switch (kind) {
        .text => "Message",
        .thinking => "Thinking",
        .tool_use => "Tool input",
        .tool_result => "Tool result",
        .code => "Code",
        .@"error" => "Error",
    };
}

fn boundedText(text: []const u8, cap: usize) []const u8 {
    var end = @min(text.len, cap);
    while (end > 0 and !std.unicode.utf8ValidateSlice(text[0..end])) : (end -= 1) {}
    return text[0..end];
}

fn boundedLineCount(text: []const u8, width: u16, cap: usize) u16 {
    if (width == 0) return 1;
    const clipped = boundedText(text, cap);
    var total: u16 = 0;
    var lines = std.mem.splitScalar(u8, clipped, '\n');
    while (lines.next()) |line| {
        total +|= @max(@as(u16, 1), @as(u16, @intCast((buffer_mod.stringWidth(line) + width - 1) / width)));
    }
    return @max(total, 1);
}

fn codeLineCount(text: []const u8, width: u16) u16 {
    return boundedLineCount(text, width -| 1, max_tool_detail);
}
