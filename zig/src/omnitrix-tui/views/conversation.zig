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
    allocator: std.mem.Allocator,
    arena: std.heap.ArenaAllocator,
    blocks: std.ArrayList(Block),
    max_blocks: usize = 512,
    max_bytes: usize = 256 * 1024,
    max_block_bytes: usize = 16 * 1024,
    retained_bytes: usize = 0,

    pub fn init(parent: std.mem.Allocator) BlockStore {
        return .{
            .allocator = parent,
            .arena = std.heap.ArenaAllocator.init(parent),
            .blocks = .empty,
        };
    }

    pub fn deinit(self: *BlockStore) void {
        self.blocks.deinit(self.allocator);
        self.arena.deinit();
    }

    pub fn append(self: *BlockStore, block: Block) !void {
        if (self.max_blocks == 0 or self.max_bytes == 0) return;

        const copy_cap = @min(self.max_block_bytes, self.max_bytes);
        const source = boundedText(block.text, copy_cap);
        while (self.blocks.items.len > 0 and
            (self.blocks.items.len >= self.max_blocks or
                self.retained_bytes > self.max_bytes -| source.len))
        {
            try self.rebuildRetained(1);
        }

        const owned_text = try self.arena.allocator().dupe(u8, source);
        var owned = block;
        owned.text = owned_text;
        owned.committed_len = if (block.is_streaming) safePrefixLen(source, block.committed_len) else source.len;
        try self.blocks.append(self.allocator, owned);
        self.retained_bytes += owned_text.len;
    }

    /// Append owned bytes to a live streaming block without exposing caller storage.
    pub fn appendStreamingChunk(self: *BlockStore, id: u32, chunk: []const u8) !bool {
        if (self.max_blocks == 0 or self.max_bytes == 0 or self.max_block_bytes == 0) return false;

        var target_index: ?usize = null;
        for (self.blocks.items, 0..) |block, i| {
            if (block.id == id) {
                if (!block.is_streaming) return false;
                target_index = i;
                break;
            }
        }
        const index = target_index orelse return false;
        const current = self.blocks.items[index];
        const copy_cap = @min(self.max_block_bytes, self.max_bytes);
        const current_len = @min(current.text.len, copy_cap);
        const append_len = @min(chunk.len, copy_cap - current_len);
        if (append_len == 0) return false;

        const candidate = try self.allocator.alloc(u8, current_len + append_len);
        defer self.allocator.free(candidate);
        @memcpy(candidate[0..current_len], current.text[0..current_len]);
        @memcpy(candidate[current_len..][0..append_len], chunk[0..append_len]);
        const committed_len = safePrefixLen(candidate, current.committed_len);
        try self.rebuildWithStreamingText(index, candidate, committed_len);
        return true;
    }

    fn rebuildWithStreamingText(self: *BlockStore, target_index: usize, new_text: []const u8, committed_len: usize) !void {
        const count = self.blocks.items.len;
        var keep = try self.allocator.alloc(bool, count);
        defer self.allocator.free(keep);
        @memset(keep, false);
        keep[target_index] = true;

        var kept_count: usize = 1;
        var kept_bytes = new_text.len;
        var i = count;
        while (i > 0 and kept_count < self.max_blocks) {
            i -= 1;
            if (i == target_index) continue;
            const block_len = self.blocks.items[i].text.len;
            if (block_len <= self.max_bytes -| kept_bytes) {
                keep[i] = true;
                kept_count += 1;
                kept_bytes += block_len;
            }
        }

        var metadata = try self.allocator.alloc(Block, kept_count);
        defer self.allocator.free(metadata);
        var snapshot = try self.allocator.alloc(u8, kept_bytes);
        defer self.allocator.free(snapshot);

        var metadata_index: usize = 0;
        var offset: usize = 0;
        for (self.blocks.items, 0..) |block, block_index| {
            if (!keep[block_index]) continue;
            var retained = block;
            const source = if (block_index == target_index) new_text else block.text;
            @memcpy(snapshot[offset..][0..source.len], source);
            retained.text = snapshot[offset..][0..source.len];
            if (block_index == target_index) retained.committed_len = safePrefixLen(source, committed_len);
            metadata[metadata_index] = retained;
            metadata_index += 1;
            offset += source.len;
        }

        _ = self.arena.reset(.retain_capacity);
        self.blocks.clearRetainingCapacity();
        self.retained_bytes = 0;
        for (metadata) |block| {
            var rebuilt = block;
            rebuilt.text = try self.arena.allocator().dupe(u8, block.text);
            try self.blocks.append(self.allocator, rebuilt);
            self.retained_bytes += rebuilt.text.len;
        }
    }

    fn rebuildRetained(self: *BlockStore, first_index: usize) !void {
        const first = @min(first_index, self.blocks.items.len);
        const retained_count = self.blocks.items.len - first;
        var metadata = try self.allocator.alloc(Block, retained_count);
        defer self.allocator.free(metadata);
        var snapshot = try self.allocator.alloc(u8, self.retained_bytes);
        defer self.allocator.free(snapshot);

        var offset: usize = 0;
        for (self.blocks.items[first..], 0..) |block, i| {
            var retained = block;
            const len = block.text.len;
            @memcpy(snapshot[offset..][0..len], block.text);
            retained.text = snapshot[offset..][0..len];
            metadata[i] = retained;
            offset += len;
        }

        _ = self.arena.reset(.retain_capacity);
        self.blocks.clearRetainingCapacity();
        self.retained_bytes = 0;
        for (metadata) |block| {
            var rebuilt = block;
            rebuilt.text = try self.arena.allocator().dupe(u8, block.text);
            try self.blocks.append(self.allocator, rebuilt);
            self.retained_bytes += rebuilt.text.len;
        }
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

    pub fn setCommittedPrefix(self: *BlockStore, id: u32, committed_len: usize) bool {
        for (self.blocks.items) |*block| {
            if (block.id == id) {
                block.committed_len = safePrefixLen(block.text, committed_len);
                return true;
            }
        }
        return false;
    }

    pub fn finishStreaming(self: *BlockStore, id: u32) bool {
        for (self.blocks.items) |*block| {
            if (block.id == id) {
                block.is_streaming = false;
                block.committed_len = block.text.len;
                return true;
            }
        }
        return false;
    }

    pub fn safePrefixLen(text: []const u8, requested: usize) usize {
        var end = @min(requested, text.len);
        while (end > 0 and !std.unicode.utf8ValidateSlice(text[0..end])) end -= 1;
        return end;
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
    const layout_markdown = MarkdownRenderer.init(Theme.dark);
    for (store.blocks.items) |block| {
        const height = blockHeight(layout_markdown, block, rect.width -| 4);
        if (content_y >= row and content_y < row + height) return block.id;
        row +|= height;
    }
    return null;
}

fn blockHeight(markdown: MarkdownRenderer, block: Block, width: u16) u16 {
    const detail_width = blockDetailWidth(block, width);
    if (detail_width == 0) return 1;
    const text = BlockStore.safeText(block);
    if (block.kind == .tool_use or block.kind == .tool_result) {
        return if (block.collapsed) 1 else 1 + markdown.measureHeight(detail_width, boundedText(text, max_tool_detail));
    }
    if (block.kind == .code) return 1 + codeLineCount(text, detail_width);
    return markdown.measureHeight(detail_width, text);
}

fn blockDetailOffset(block: Block) u16 {
    const marker_width: u16 = if (block.kind == .tool_use or block.kind == .tool_result) 1 else 0;
    return marker_width +| buffer_mod.stringWidth(kindLabel(block.kind)) +| 1;
}

fn blockDetailWidth(block: Block, width: u16) u16 {
    return width -| @min(blockDetailOffset(block), width);
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

    const detail_offset = @min(blockDetailOffset(block), width);
    const detail_x = x +| detail_offset;
    const detail_width = width - detail_offset;
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
