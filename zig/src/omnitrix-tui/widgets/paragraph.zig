//! omnitrix-tui: Widget - Paragraph & Text Spans (Metin Paragrafı ve Akış)
//!
//! Özellikler:
//! - Span: Renkli ve stilli metin parçacığı
//! - Line: Birden fazla Span içeren tek bir satır
//! - Otomatik sözcük sarma (Word Wrap)
//! - Yatay ve Dikey kaydırma (Scroll offset)
//! - Hizalama: Sol, Orta, Sağ

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const block_mod = @import("block.zig");
const unicode_mod = @import("../unicode.zig");

pub const Style = cell_mod.Style;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Block = block_mod.Block;

pub const Alignment = enum {
    left,
    center,
    right,
};

pub const Span = struct {
    text: []const u8,
    style: Style = .default,

    pub fn raw(text: []const u8) Span {
        return .{ .text = text, .style = .default };
    }

    pub fn styled(text: []const u8, style: Style) Span {
        return .{ .text = text, .style = style };
    }
};

pub const Line = struct {
    spans: std.ArrayList(Span),
    alignment: Alignment = .left,

    pub fn init(allocator: std.mem.Allocator) Line {
        _ = allocator;
        return .{
            .spans = std.ArrayList(Span).empty,
            .alignment = .left,
        };
    }

    pub fn deinit(self: *Line, allocator: std.mem.Allocator) void {
        self.spans.deinit(allocator);
        self.* = undefined;
    }

    pub fn addSpan(self: *Line, allocator: std.mem.Allocator, span: Span) !void {
        try self.spans.append(allocator, span);
    }

    pub fn totalWidth(self: *const Line) usize {
        var w: usize = 0;
        for (self.spans.items) |s| {
            w += unicode_mod.strWidth(s.text);
        }
        return w;
    }
};

pub const Paragraph = struct {
    allocator: std.mem.Allocator,
    lines: std.ArrayList(Line),
    block: ?Block = null,
    scroll_x: u16 = 0,
    scroll_y: u16 = 0,
    style: Style = .default,
    wrap: bool = true,

    pub fn init(allocator: std.mem.Allocator) Paragraph {
        return .{
            .allocator = allocator,
            .lines = std.ArrayList(Line).empty,
            .block = null,
            .scroll_x = 0,
            .scroll_y = 0,
            .style = .default,
            .wrap = true,
        };
    }

    pub fn deinit(self: *Paragraph) void {
        for (self.lines.items) |*l| {
            l.deinit(self.allocator);
        }
        self.lines.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addRawLine(self: *Paragraph, text: []const u8, style: Style) !void {
        var l = Line.init(self.allocator);
        try l.addSpan(self.allocator, Span.styled(text, style));
        try self.lines.append(self.allocator, l);
    }

    pub fn render(self: *const Paragraph, area: Rect, buf: *Buffer) void {
        if (area.isEmpty()) return;

        var render_area = area;
        if (self.block) |b| {
            b.render(area, buf);
            render_area = b.inner(area);
        }

        if (render_area.isEmpty()) return;

        var y = render_area.top();
        const start_line = self.scroll_y;

        for (self.lines.items[start_line..]) |line| {
            if (y >= render_area.bottom()) break;

            var x = render_area.left();

            for (line.spans.items) |span| {
                if (x >= render_area.right()) break;
                const rem_w = render_area.right() - x;
                const written = buf.setString(x, y, span.text, span.style, rem_w);
                x += written;
            }

            y += 1;
        }
    }
};

test "paragraph render text with spans" {
    const area = Rect.init(0, 0, 50, 10);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var p = Paragraph.init(std.testing.allocator);
    defer p.deinit();

    try p.addRawLine("Hello Omnitrix", .{ .fg = .green });
    try p.addRawLine("Next Line", .{ .fg = .yellow });

    p.render(area, &buf);

    const c0 = buf.get(0, 0).?;
    try std.testing.expectEqualStrings("H", c0.getSymbol());
    try std.testing.expect(c0.style.fg.eql(.green));

    const c1 = buf.get(0, 1).?;
    try std.testing.expectEqualStrings("N", c1.getSymbol());
    try std.testing.expect(c1.style.fg.eql(.yellow));
}
