const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const stringWidth = buffer_mod.stringWidth;

/// Markdown element types for TUI rendering
pub const MdElement = union(enum) {
    heading: struct { level: u8, text: []const u8 },
    paragraph: []const u8,
    code_block: struct { lang: []const u8, code: []const u8 },
    inline_code: []const u8,
    bold: []const u8,
    italic: []const u8,
    link: struct { text: []const u8, url: []const u8 },
    bullet_item: []const u8,
    numbered_item: struct { number: u16, text: []const u8 },
    blockquote: []const u8,
    horizontal_rule,
    line_break,
    text: []const u8,
};

/// Parse and render simple markdown to a TUI buffer
pub const MarkdownRenderer = struct {
    theme: Theme,
    indent: u16,

    pub fn init(theme: Theme) MarkdownRenderer {
        return .{ .theme = theme, .indent = 0 };
    }
    fn trimLeadingSpaces(text: []const u8) []const u8 {
        var start: usize = 0;
        while (start < text.len and text[start] == ' ') start += 1;
        return text[start..];
    }

    fn trimSpaces(text: []const u8) []const u8 {
        const leading = trimLeadingSpaces(text);
        var end = leading.len;
        while (end > 0 and leading[end - 1] == ' ') end -= 1;
        return leading[0..end];
    }

    /// Render markdown text into buffer starting at (x, y)
    /// Returns the y position after rendering
    pub fn render(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8) u16 {
        var lines = std.mem.splitScalar(u8, text, '\n');
        var current_y = y;
        var in_code_block = false;

        while (lines.next()) |line| {
            if (current_y >= buf.rows) break; // safety

            // Code block toggle
            if (std.mem.startsWith(u8, trimLeadingSpaces(line), "```")) {
                if (in_code_block) {
                    // End code block - render accumulated content
                    in_code_block = false;
                } else {
                    in_code_block = true;
                }
                continue;
            }

            if (in_code_block) {
                current_y = self.renderCodeLine(buf, x, current_y, width, line);
                continue;
            }

            const trimmed = trimLeadingSpaces(line);

            // Heading
            if (trimmed.len > 0 and trimmed[0] == '#') {
                var level: u8 = 0;
                for (trimmed) |ch| {
                    if (ch == '#') level += 1 else break;
                }
                if (level >= 1 and level <= 6 and trimmed.len > level and trimmed[level] == ' ') {
                    const heading_text = trimSpaces(trimmed[level + 1 ..]);
                    current_y = self.renderHeading(buf, x, current_y, width, heading_text, level);
                    continue;
                }
            }

            // Horizontal rule
            if (trimmed.len >= 3 and
                (std.mem.eql(u8, trimmed, "---") or std.mem.eql(u8, trimmed, "***") or std.mem.eql(u8, trimmed, "___")))
            {
                self.renderHr(buf, x, current_y, width);
                current_y += 1;
                continue;
            }

            // Blockquote
            if (std.mem.startsWith(u8, trimmed, "> ")) {
                const quote_text = trimmed[2..];
                current_y = self.renderBlockquote(buf, x + 2, current_y, width -| 2, quote_text);
                continue;
            }

            // Bullet list
            if (std.mem.startsWith(u8, trimmed, "- ") or std.mem.startsWith(u8, trimmed, "* ")) {
                const item_text = trimmed[2..];
                current_y = self.renderBulletItem(buf, x + 2, current_y, width -| 2, item_text);
                continue;
            }

            // Numbered list
            {
                var num: u16 = 0;
                var rest: []const u8 = "";
                for (trimmed, 0..) |ch, i| {
                    if (ch >= '0' and ch <= '9') {
                        num = num * 10 + @as(u16, ch - '0');
                    } else if (ch == '.' and i > 0 and trimmed[i - 1] >= '0' and trimmed[i - 1] <= '9') {
                        rest = trimLeadingSpaces(trimmed[i + 1 ..]);
                        break;
                    } else {
                        num = 0;
                        break;
                    }
                }
                if (num > 0 and rest.len > 0) {
                    current_y = self.renderNumberedItem(buf, x + 2, current_y, width -| 2, num, rest);
                    continue;
                }
            }

            // Empty line
            if (trimmed.len == 0) {
                current_y += 1;
                continue;
            }

            // Regular paragraph with inline formatting
            current_y = self.renderParagraph(buf, x, current_y, width, trimmed);
        }

        return current_y;
    }

    fn renderHeading(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8, level: u8) u16 {
        const style = switch (level) {
            1 => Style{ .fg = self.theme.syntax_keyword, .bg = self.theme.background, .attr = .{ .bold = true } },
            2 => Style{ .fg = self.theme.syntax_function, .bg = self.theme.background, .attr = .{ .bold = true } },
            3 => Style{ .fg = self.theme.syntax_type, .bg = self.theme.background, .attr = .{ .bold = true } },
            else => Style{ .fg = self.theme.text, .bg = self.theme.background, .attr = .{ .bold = true, .dim = true } },
        };

        // Render heading with underline on h1/h2
        _ = buf.writeStringBounded(x, y, text, style, width);
        if (level <= 2) {
            const underline_char: u21 = if (level == 1) '━' else '─';
            var underline_style = style;
            underline_style.attr.bold = false;
            underline_style.attr.dim = true;
            for (0..@min(stringWidth(text), width)) |i| {
                buf.setCell(x + @as(u16, @intCast(i)), y + 1, .{
                    .char = .{ .char = underline_char },
                    .style = underline_style,
                });
            }
            return y + 2;
        }
        return y + 1;
    }

    fn renderCodeLine(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, line: []const u8) u16 {
        const code_style = Style{ .fg = self.theme.syntax_string, .bg = self.theme.background_panel };
        // Code background
        for (0..width) |i| {
            buf.setCell(x + @as(u16, @intCast(i)), y, .{ .style = code_style });
        }
        _ = buf.writeStringBounded(x + 1, y, line, code_style, width -| 1);
        return y + 1;
    }

    fn renderBlockquote(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8) u16 {
        const quote_style = Style{ .fg = self.theme.text_muted, .bg = self.theme.background };
        // Vertical bar
        buf.setCell(x - 2, y, .{ .char = .{ .char = '│' }, .style = Style{ .fg = self.theme.border_active, .bg = self.theme.background } });
        _ = buf.writeStringBounded(x, y, text, quote_style, width);
        return y + 1;
    }

    fn renderBulletItem(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8) u16 {
        buf.setCell(x - 2, y, .{ .char = .{ .char = '•' }, .style = Style{ .fg = self.theme.accent, .bg = self.theme.background } });
        return self.renderInline(buf, x, y, width, text, self.theme.fgStyle(self.theme.text));
    }

    fn renderNumberedItem(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, num: u16, text: []const u8) u16 {
        var num_buf: [8]u8 = undefined;
        const num_str = std.fmt.bufPrint(&num_buf, "{d}. ", .{num}) catch "   ";
        const num_style = Style{ .fg = self.theme.accent, .bg = self.theme.background };
        _ = buf.writeStringBounded(x - @as(u16, @intCast(num_str.len)), y, num_str, num_style, @as(u16, @intCast(num_str.len)));
        return self.renderInline(buf, x, y, width, text, self.theme.fgStyle(self.theme.text));
    }

    fn renderHr(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16) void {
        const style = Style{ .fg = self.theme.border, .bg = self.theme.background };
        for (0..width) |i| {
            buf.setCell(x + @as(u16, @intCast(i)), y, .{ .char = .{ .char = '─' }, .style = style });
        }
    }

    pub fn measureHeight(self: MarkdownRenderer, width: u16, text: []const u8) u16 {
        _ = self;
        if (width == 0) return 1;
        var height: u16 = 0;
        var lines = std.mem.splitScalar(u8, text, '\n');
        while (lines.next()) |line| {
            if (line.len == 0) {
                height +|= 1;
                continue;
            }
            var used: u16 = 0;
            var words = std.mem.splitScalar(u8, line, ' ');
            while (words.next()) |word| {
                const word_width = stringWidth(word);
                if (used > 0 and used + 1 + word_width > width) {
                    height +|= 1;
                    used = 0;
                }
                used +|= if (used == 0) word_width else 1 + word_width;
                if (used > width) used = width;
            }
            height +|= 1;
        }
        return @max(height, 1);
    }

    fn renderParagraph(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8) u16 {
        return self.renderInline(buf, x, y, width, text, self.theme.fgStyle(self.theme.text));
    }

    /// Render markdown with word-aware line breaks and hidden inline markers.
    fn renderInline(self: MarkdownRenderer, buf: *Buffer, start_x: u16, y: u16, width: u16, text: []const u8, base_style: Style) u16 {
        if (width == 0) return y + 1;
        var line_y = y;
        var line_x = start_x;
        var words = std.mem.splitScalar(u8, text, ' ');
        while (words.next()) |word| {
            const word_width = stringWidth(word);
            if (line_x > start_x and line_x - start_x + 1 + word_width > width) {
                line_y += 1;
                line_x = start_x;
            }
            line_x = self.renderInlineToken(buf, line_x, line_y, width -| (line_x - start_x), word, base_style);
            if (line_x < start_x + width) {
                buf.setCell(line_x, line_y, .{ .char = .{ .char = ' ' }, .style = base_style });
                line_x += 1;
            } else {
                line_y += 1;
                line_x = start_x;
            }
        }
        while (line_x < start_x + width) {
            buf.setCell(line_x, line_y, .{ .style = base_style });
            line_x += 1;
        }
        return line_y + 1;
    }

    fn renderInlineToken(self: MarkdownRenderer, buf: *Buffer, start_x: u16, y: u16, width: u16, text: []const u8, base_style: Style) u16 {
        var x = start_x;
        var i: usize = 0;
        while (i < text.len and x - start_x < width) {
            if (std.mem.startsWith(u8, text[i..], "**")) {
                if (std.mem.indexOf(u8, text[i + 2 ..], "**")) |end| {
                    var style = base_style;
                    style.attr.bold = true;
                    x = renderStyled(buf, x, y, width -| (x - start_x), text[i + 2 .. i + 2 + end], style);
                    i += 2 + end + 2;
                    continue;
                }
            }
            if (text[i] == '`') {
                if (std.mem.indexOfScalar(u8, text[i + 1 ..], '`')) |end| {
                    var style = base_style;
                    style.fg = self.theme.syntax_string;
                    style.bg = self.theme.background_panel;
                    x = renderStyled(buf, x, y, width -| (x - start_x), text[i + 1 .. i + 1 + end], style);
                    i += 1 + end + 1;
                    continue;
                }
            }
            if (text[i] == '*' or text[i] == '_') {
                i += 1;
                continue;
            }
            if (text[i] == '[') {
                if (std.mem.indexOfScalar(u8, text[i + 1 ..], ']')) |close| {
                    const after = i + 1 + close;
                    if (after + 1 < text.len and text[after + 1] == '(') {
                        if (std.mem.indexOfScalar(u8, text[after + 2 ..], ')')) |url_end| {
                            var style = base_style;
                            style.fg = self.theme.accent;
                            style.attr.underline = true;
                            x = renderStyled(buf, x, y, width -| (x - start_x), text[i + 1 .. after], style);
                            i = after + 3 + url_end;
                            continue;
                        }
                    }
                }
            }
            const seq = std.unicode.utf8ByteSequenceLength(text[i]) catch 1;
            const n = @min(@as(usize, seq), text.len - i);
            const cp = std.unicode.wtf8Decode(text[i .. i + n]) catch @as(u21, text[i]);
            const cw = buffer_mod.charWidth(cp);
            if (cw > 0 and x - start_x + cw <= width) {
                buf.setCell(x, y, .{ .char = .{ .char = cp }, .style = base_style, .width = cw });
                if (cw == 2) buf.setCell(x + 1, y, .{ .char = .wide_right, .style = base_style, .width = 0 });
                x += cw;
            }
            i += n;
        }
        return x;
    }

    fn renderStyled(buf: *Buffer, start_x: u16, y: u16, width: u16, text: []const u8, style: Style) u16 {
        var x = start_x;
        var i: usize = 0;
        while (i < text.len and x - start_x < width) {
            const seq = std.unicode.utf8ByteSequenceLength(text[i]) catch 1;
            const n = @min(@as(usize, seq), text.len - i);
            const cp = std.unicode.wtf8Decode(text[i .. i + n]) catch @as(u21, text[i]);
            const cw = buffer_mod.charWidth(cp);
            if (cw > 0 and x - start_x + cw <= width) {
                buf.setCell(x, y, .{ .char = .{ .char = cp }, .style = style, .width = cw });
                if (cw == 2) buf.setCell(x + 1, y, .{ .char = .wide_right, .style = style, .width = 0 });
                x += cw;
            }
            i += n;
        }
        return x;
    }
};
