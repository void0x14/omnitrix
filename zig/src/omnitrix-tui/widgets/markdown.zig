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

    /// Render markdown text into buffer starting at (x, y)
    /// Returns the y position after rendering
    pub fn render(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8) u16 {
        var lines = std.mem.splitScalar(u8, text, '\n');
        var current_y = y;
        var in_code_block = false;
        var code_lang: []const u8 = "";
        var code_content_start: usize = 0;

        while (lines.next()) |line| {
            if (current_y >= y + width) break; // safety

            // Code block toggle
            if (std.mem.startsWith(u8, std.mem.trimLeft(u8, line, " "), "```")) {
                if (in_code_block) {
                    // End code block - render accumulated content
                    in_code_block = false;
                } else {
                    in_code_block = true;
                    code_lang = std.mem.trimLeft(u8, line[3..], " ");
                    code_content_start = 0;
                }
                continue;
            }

            if (in_code_block) {
                current_y = self.renderCodeLine(buf, x, current_y, width, line);
                continue;
            }

            const trimmed = std.mem.trimLeft(u8, line, " ");

            // Heading
            if (trimmed.len > 0 and trimmed[0] == '#') {
                var level: u8 = 0;
                for (trimmed) |ch| {
                    if (ch == '#') level += 1 else break;
                }
                if (level >= 1 and level <= 6 and trimmed.len > level and trimmed[level] == ' ') {
                    const heading_text = std.mem.trim(u8, trimmed[level + 1 ..], " ");
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
                        rest = std.mem.trimLeft(u8, trimmed[i + 1 ..], " ");
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

    fn renderParagraph(self: MarkdownRenderer, buf: *Buffer, x: u16, y: u16, width: u16, text: []const u8) u16 {
        return self.renderInline(buf, x, y, width, text, self.theme.fgStyle(self.theme.text));
    }

    /// Render text with inline formatting (bold, italic, code, links)
    fn renderInline(self: MarkdownRenderer, buf: *Buffer, start_x: u16, y: u16, width: u16, text: []const u8, base_style: Style) u16 {
        var x = start_x;
        var i: usize = 0;
        var current_style = base_style;

        while (i < text.len and (x - start_x) < width) {
            // Inline code: `code`
            if (text[i] == '`' and i + 1 < text.len) {
                const end = std.mem.indexOfScalar(u8, text[i + 1 ..], '`') orelse text.len;
                const code_text = text[i + 1 ..][0..end];
                var code_style = base_style;
                code_style.fg = self.theme.syntax_string;
                code_style.bg = self.theme.background_panel;
                for (code_text) |ch| {
                    if (x - start_x >= width) break;
                    buf.setCell(x, y, .{ .char = .{ .char = ch }, .style = code_style });
                    x += 1;
                }
                i = i + 1 + end + 1;
                current_style = base_style;
                continue;
            }

            // Bold: **text** or __text__
            if (text[i] == '*' and i + 1 < text.len and text[i + 1] == '*') {
                if (std.mem.indexOfScalar(u8, text[i + 2 ..], "**")) |end| {
                    const bold_text = text[i + 2 ..][0..end];
                    var bold_style = base_style;
                    bold_style.attr.bold = true;
                    for (bold_text) |ch| {
                        if (x - start_x >= width) break;
                        buf.setCell(x, y, .{ .char = .{ .char = ch }, .style = bold_style });
                        x += 1;
                    }
                    i = i + 2 + end + 2;
                    current_style = base_style;
                    continue;
                }
            }

            // Italic: *text* or _text_
            if (text[i] == '*' and (i + 1 < text.len and text[i + 1] != '*')) {
                if (std.mem.indexOfScalar(u8, text[i + 1 ..], "*")) |end| {
                    const italic_text = text[i + 1 ..][0..end];
                    var italic_style = base_style;
                    italic_style.attr.italic = true;
                    for (italic_text) |ch| {
                        if (x - start_x >= width) break;
                        buf.setCell(x, y, .{ .char = .{ .char = ch }, .style = italic_style });
                        x += 1;
                    }
                    i = i + 1 + end + 1;
                    current_style = base_style;
                    continue;
                }
            }

            // Link: [text](url)
            if (text[i] == '[') {
                if (std.mem.indexOfScalar(u8, text[i + 1 ..], ']')) |close_idx| {
                    const link_text = text[i + 1 ..][0..close_idx];
                    const rest = text[i + 1 + close_idx ..];
                    if (rest.len > 0 and rest[0] == '(') {
                        if (std.mem.indexOfScalar(u8, rest[1..], ')')) |url_end| {
                            // Just render the link text with accent color
                            var link_style = base_style;
                            link_style.fg = self.theme.accent;
                            link_style.attr.underline = true;
                            for (link_text) |ch| {
                                if (x - start_x >= width) break;
                                buf.setCell(x, y, .{ .char = .{ .char = ch }, .style = link_style });
                                x += 1;
                            }
                            i = i + 1 + close_idx + 1 + url_end + 2; // skip [text](url)
                            current_style = base_style;
                            continue;
                        }
                    }
                }
            }

            // Regular character
            if (x - start_x < width) {
                buf.setCell(x, y, .{ .char = .{ .char = text[i] }, .style = current_style });
                x += 1;
            }
            i += 1;
        }

        // Fill rest of line
        while ((x - start_x) < width) {
            buf.setCell(x, y, .{ .style = base_style });
            x += 1;
        }

        return y + 1;
    }
};
