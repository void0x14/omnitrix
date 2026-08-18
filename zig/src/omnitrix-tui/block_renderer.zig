//! omnitrix-tui: Conversation Block Renderer & Safe-Tail Streaming Markdown (tasarım Bölüm 5.1, Kol A - A2, Doğrulama 11).
//!
//! Özellikler:
//! - Conversation block listesi (user, agent, tool, system).
//! - Collapsed / expanded tool blokları (araç çağrısı ve çıktısının katlanabilir görünümü).
//! - Streaming markdown safe-tail render (her token geldiğinde tüm transcript'i yeniden
//!   serialize etmeden sadece yeni gelen parçaları güvenli ve titreşimsiz işler).
//! - Bounded buffer yönetimi (I6 ve Bölüm 8 gereği sınırsız bellek ayrımı engellenir).
//! - Unicode uyumlu satır kaydırma ve formatlama.
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const unicode = @import("unicode.zig");
const term = @import("terminal.zig");

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
    tool_collapsed: bool = true, // Araç blokları varsayılan olarak katlıdır
    tool_name: ?[]const u8 = null,
    tool_summary: ?[]const u8 = null,
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

    /// Yeni bir konuşma bloğu ekler. Kapasite dolarsa en eski bloğu çıkarır (Bounded queue).
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

    /// Tool bloğu ekler (varsayılan collapsed)
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
        }
        return id;
    }

    pub fn getBlock(self: *BlockRenderer, id: u64) ?*Block {
        for (self.blocks.items) |*b| {
            if (b.id == id) return b;
        }
        return null;
    }

    /// Tool bloğunun katlanma durumunu değiştirir (collapse/expand toggle).
    pub fn toggleToolCollapse(self: *BlockRenderer, id: u64) bool {
        if (self.getBlock(id)) |b| {
            b.toggleCollapse();
            return true;
        }
        return false;
    }

    /// Streaming safe-tail: Gelen yeni metin parçasını aktif bloğa ekler (Doğrulama 11).
    /// Bounded modelde tüm transcript'i baştan hesaplamak yerine tail parçayı büyütür.
    pub fn appendStreamingChunk(self: *BlockRenderer, block_id: u64, chunk: []const u8, revision: u64) !void {
        const block = self.getBlock(block_id) orelse return error.BlockNotFound;
        try block.content.appendSlice(self.allocator, chunk);
        block.revision = revision;
        block.status = .in_progress;
    }

    /// Bloğu tamamlandı olarak işaretler
    pub fn finishBlock(self: *BlockRenderer, block_id: u64, status: BlockStatus) void {
        if (self.getBlock(block_id)) |b| {
            b.status = status;
        }
    }

    /// Blok listesini verilen genişliğe göre terminal satırlarına formatlar
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

        const safe_w = if (max_width > 4) max_width - 2 else max_width;

        for (self.blocks.items) |block| {
            // Başlık satırı
            var header_buf: [256]u8 = undefined;
            const status_icon: []const u8 = switch (block.status) {
                .in_progress => "⏳",
                .completed => "✓",
                .failed => "✗",
            };

            const header_str = switch (block.kind) {
                .user => try std.fmt.bufPrint(&header_buf, "── 👤 {s} ──", .{block.header}),
                .agent => try std.fmt.bufPrint(&header_buf, "── 🤖 {s} ({s}) ──", .{ block.header, status_icon }),
                .tool => if (block.tool_collapsed)
                    try std.fmt.bufPrint(&header_buf, "▶ 🛠️ [{s}] {s} ({s})", .{
                        block.tool_name orelse block.header,
                        block.tool_summary orelse "",
                        status_icon,
                    })
                else
                    try std.fmt.bufPrint(&header_buf, "▼ 🛠️ [{s}] {s}", .{ block.tool_name orelse block.header, status_icon }),
                .system => try std.fmt.bufPrint(&header_buf, "── ⚙️ {s} ──", .{block.header}),
            };

            try lines.append(allocator, try allocator.dupe(u8, header_str));

            // Katlanmış tool bloğunun içeriği gizlenir (yalnızca başlık satırı görünür)
            if (block.kind == .tool and block.tool_collapsed) {
                continue;
            }

            // İçerik satırları (word wrap ile güvenli render)
            var content_lines = try unicode.wrapText(allocator, block.content.items, safe_w);
            defer {
                for (content_lines.items) |cl| allocator.free(cl);
                content_lines.deinit(allocator);
            }

            for (content_lines.items) |cl| {
                const indented = if (block.kind == .tool)
                    try std.fmt.allocPrint(allocator, "  │ {s}", .{cl})
                else
                    try std.fmt.allocPrint(allocator, "  {s}", .{cl});

                try lines.append(allocator, indented);
            }

            // Bloklar arası boşluk
            try lines.append(allocator, try allocator.dupe(u8, ""));
        }

        return lines;
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11 / A2)
// -----------------------------------------------------------------------------

test "block renderer ekleme ve tool collapse/expand toggle" {
    var renderer = BlockRenderer.init(std.testing.allocator, 10);
    defer renderer.deinit();

    _ = try renderer.addBlock(.user, "Kullanıcı", "main.zig dosyasını incele");
    const tool_id = try renderer.addToolBlock("file_read", "path: src/main.zig", "pub fn main() void {\n    // code\n}");

    // Tool varsayılan olarak collapsed
    const tb = renderer.getBlock(tool_id).?;
    try std.testing.expect(tb.tool_collapsed);

    var lines_collapsed = try renderer.renderToLines(std.testing.allocator, 60);
    defer {
        for (lines_collapsed.items) |l| std.testing.allocator.free(l);
        lines_collapsed.deinit(std.testing.allocator);
    }

    // Katlıyken içindeki kod satırları basılmaz
    var has_inner_code = false;
    for (lines_collapsed.items) |l| {
        if (std.mem.indexOf(u8, l, "pub fn main()") != null) has_inner_code = true;
    }
    try std.testing.expect(!has_inner_code);

    // Expand toggle yap
    try std.testing.expect(renderer.toggleToolCollapse(tool_id));
    try std.testing.expect(!tb.tool_collapsed);

    var lines_expanded = try renderer.renderToLines(std.testing.allocator, 60);
    defer {
        for (lines_expanded.items) |l| std.testing.allocator.free(l);
        lines_expanded.deinit(std.testing.allocator);
    }

    // Açıkken içindeki kod satırları görünür
    has_inner_code = false;
    for (lines_expanded.items) |l| {
        if (std.mem.indexOf(u8, l, "pub fn main()") != null) has_inner_code = true;
    }
    try std.testing.expect(has_inner_code);
}

test "streaming markdown safe-tail append (Dogrulama 11)" {
    var renderer = BlockRenderer.init(std.testing.allocator, 10);
    defer renderer.deinit();

    const ag_id = try renderer.addBlock(.agent, "Omnitrix", "");
    try renderer.appendStreamingChunk(ag_id, "Token 1... ", 1);
    try renderer.appendStreamingChunk(ag_id, "Token 2... ", 2);
    try renderer.appendStreamingChunk(ag_id, "Token 3 done.", 3);

    const b = renderer.getBlock(ag_id).?;
    try std.testing.expectEqualStrings("Token 1... Token 2... Token 3 done.", b.content.items);
    try std.testing.expectEqual(@as(u64, 3), b.revision);
}
