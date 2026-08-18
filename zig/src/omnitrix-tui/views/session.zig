const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const theme_mod = @import("../core/theme.zig");
const layout_mod = @import("../core/layout.zig");
const scroll_mod = @import("../widgets/scroll.zig");
const textarea_mod = @import("../widgets/textarea.zig");
const text_mod = @import("../widgets/text.zig");
const markdown_mod = @import("../widgets/markdown.zig");
const spinner_mod = @import("../widgets/spinner.zig");
const sidebar_mod = @import("sidebar.zig");
const footer_mod = @import("footer.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Color = cell_mod.Color;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const Rect = layout_mod.Rect;
const ScrollContainer = scroll_mod.ScrollContainer;
const TextareaWidget = textarea_mod.TextareaWidget;
const TextWidget = text_mod.TextWidget;
const MarkdownRenderer = markdown_mod.MarkdownRenderer;
const Spinner = spinner_mod.Spinner;
const SidebarInfo = sidebar_mod.SidebarInfo;
const FooterView = footer_mod.FooterView;
const FooterInfo = footer_mod.FooterInfo;

pub const MessageRole = enum { user, assistant, system };

pub const MessagePart = union(enum) {
    text: []const u8,
    code: struct { lang: []const u8, code: []const u8 },
    tool_use: struct { name: []const u8, input: []const u8 },
    tool_result: struct { name: []const u8, output: []const u8 },
    thinking: []const u8,
    err_text: []const u8,
};

pub const Message = struct {
    role: MessageRole,
    parts: []const MessagePart,
    timestamp: u64 = 0,
    is_streaming: bool = false,
    id: []const u8 = "",
};

pub const SessionView = struct {
    allocator: std.mem.Allocator,
    messages: std.ArrayList(Message),
    scroll: ScrollContainer,
    prompt: TextareaWidget,
    markdown: MarkdownRenderer,
    spinner: Spinner,
    sidebar: SidebarView,
    footer: FooterView,
    sidebar_visible: bool,
    focused: enum { conversation, prompt },
    show_timestamps: bool,
    show_thinking: bool,
    show_tool_details: bool,

    pub const SidebarView = sidebar_mod.SidebarView;

    pub fn init(allocator: std.mem.Allocator, theme: Theme) !SessionView {
        return .{
            .allocator = allocator,
            .messages = .empty,
            .scroll = ScrollContainer.init(30, 80),
            .prompt = try TextareaWidget.init(
                allocator,
                Style{ .fg = theme.text, .bg = theme.background_panel },
                Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
            ),
            .markdown = MarkdownRenderer.init(theme),
            .spinner = Spinner.init("Thinking...", Style{ .fg = theme.accent }, Style{ .fg = theme.text_muted }),
            .sidebar = SidebarView.init(),
            .footer = FooterView.init(),
            .sidebar_visible = true,
            .focused = .prompt,
            .show_timestamps = false,
            .show_thinking = true,
            .show_tool_details = true,
        };
    }

    pub fn deinit(self: *SessionView) void {
        self.messages.deinit(self.allocator);
        self.prompt.deinit();
    }

    pub fn addMessage(self: *SessionView, msg: Message) !void {
        try self.messages.append(self.allocator, msg);
        self.scrollToBottom();
    }

    pub fn scrollToBottom(self: *SessionView) void {
        self.scroll.scrollToBottom();
    }

    pub fn toggleSidebar(self: *SessionView) void {
        self.sidebar_visible = !self.sidebar_visible;
    }

    /// Render the entire session view
    pub fn render(self: *SessionView, buf: *Buffer, terminal_width: u16, terminal_height: u16, theme: Theme) void {
        // Clear
        buf.fillRegion(0, 0, terminal_width, terminal_height, .{ .style = .{ .bg = theme.background } });

        const sidebar_w: u16 = if (self.sidebar_visible) self.sidebar.width + 2 else 0;
        const content_width = terminal_width -| sidebar_w;
        const footer_y = terminal_height - 1;
        const header_y: u16 = 0;
        const prompt_height: u16 = 4;
        const conversation_height = footer_y -| header_y -| prompt_height -| 1;

        // Header
        self.renderHeader(buf, 0, header_y, content_width, theme);

        // Conversation area
        const conv_rect = Rect{
            .x = 0,
            .y = header_y + 1,
            .width = content_width,
            .height = conversation_height,
        };
        self.renderConversation(buf, conv_rect, theme);

        // Prompt area
        const prompt_rect = Rect{
            .x = 0,
            .y = header_y + 1 + conversation_height,
            .width = content_width,
            .height = prompt_height,
        };
        self.renderPrompt(buf, prompt_rect, theme);

        // Sidebar
        if (self.sidebar_visible) {
            const sidebar_rect = Rect{
                .x = content_width,
                .y = 0,
                .width = sidebar_w,
                .height = terminal_height - 1,
            };
            self.sidebar.render(buf, sidebar_rect, theme);

            // Vertical separator
            const sep_style = Style{ .fg = theme.border, .bg = theme.background_panel };
            for (0..terminal_height - 1) |i| {
                buf.setCell(content_width, @as(u16, @intCast(i)), .{ .char = .{ .char = '│' }, .style = sep_style });
            }
        }

        // Footer
        self.footer.render(buf, footer_y, terminal_width, theme);
    }

    fn renderHeader(_: SessionView, buf: *Buffer, x: u16, y: u16, width: u16, theme: Theme) void {
        // Header background
        const header_style = Style{ .fg = theme.text, .bg = theme.background_panel, .attr = .{ .bold = true } };
        for (0..width) |i| {
            buf.setCell(x + @as(u16, @intCast(i)), y, .{ .style = header_style });
        }
        _ = buf.writeStringBounded(x + 2, y, "Omnitrix", header_style, width -| 4);

        // Right side: tab indicators
        const tabs = "[Conversation] [Changes] [Diff]";
        const tab_w = buffer_mod.stringWidth(tabs);
        if (tab_w < width - 20) {
            _ = buf.writeStringBounded(x + width - tab_w - 2, y, tabs, Style{ .fg = theme.text_muted, .bg = theme.background_panel }, tab_w);
        }
    }

    fn renderConversation(self: *SessionView, buf: *Buffer, rect: Rect, theme: Theme) void {
        // Background
        buf.fillRegion(rect.x, rect.y, rect.width, rect.height, .{ .style = .{ .bg = theme.background } });

        if (self.messages.items.len == 0) {
            // Empty state
            const empty_style = Style{ .fg = theme.text_dim, .bg = theme.background };
            const empty_msg = "No messages yet. Start typing below...";
            const ew = buffer_mod.stringWidth(empty_msg);
            const ex = if (ew < rect.width) rect.x + (rect.width - ew) / 2 else rect.x;
            const ey = rect.y + rect.height / 2;
            _ = buf.writeStringBounded(ex, ey, empty_msg, empty_style, rect.width);
            return;
        }

        var content_y: u16 = 0;
        for (self.messages.items) |msg| {
            const msg_height = self.calculateMessageHeight(msg, rect.width - 4);

            // Check if this message is visible in scroll viewport
            const screen_y = self.scroll.contentToScreen(content_y);
            if (screen_y) |sy| {
                if (sy < rect.height) {
                    self.renderMessage(buf, rect.x + 2, rect.y + sy, rect.width - 4, msg, theme);
                }
            }

            content_y += msg_height + 1; // +1 for gap between messages
        }

        self.scroll.setContentHeight(content_y);

        // Spinner if last message is streaming
        if (self.messages.items.len > 0) {
            const last = self.messages.items[self.messages.items.len - 1];
            if (last.is_streaming) {
                var sp = self.spinner;
                sp.tick();
                const sy = self.scroll.contentToScreen(content_y);
                if (sy) |screen_y| {
                    if (screen_y < rect.height) {
                        sp.render(buf, rect.x + 2, rect.y + screen_y, rect.width - 4);
                    }
                }
            }
        }

        // Scrollbar
        self.scroll.renderScrollbar(buf, rect.x + rect.width - 1, rect.y, rect.height);
    }

    fn calculateMessageHeight(self: SessionView, msg: Message, width: u16) u16 {
        _ = self;
        var height: u16 = 1; // Role header
        for (msg.parts) |part| {
            switch (part) {
                .text => |t| {
                    height += TextWidget.init(t, .{}).lineCount(width - 2);
                },
                .code => |c| {
                    height += 1; // code fence
                    var lines = std.mem.splitScalar(u8, c.code, '\n');
                    while (lines.next()) |_| height += 1;
                    height += 1; // closing fence
                },
                .tool_use => |tu| {
                    height += 1;
                    _ = tu;
                },
                .tool_result => |tr| {
                    height += 1;
                    _ = tr;
                },
                .thinking => |t| {
                    height += TextWidget.init(t, .{}).lineCount(width - 2);
                },
                .err_text => |e| {
                    height += TextWidget.init(e, .{}).lineCount(width - 2);
                },
            }
        }
        return height + 1; // padding
    }

    fn renderMessage(self: SessionView, buf: *Buffer, x: u16, y: u16, width: u16, msg: Message, theme: Theme) void {
        var current_y = y;

        // Role indicator
        const role_info = switch (msg.role) {
            .user => .{ @as([]const u8, "You"), theme.accent },
            .assistant => .{ @as([]const u8, "Assistant"), theme.success },
            .system => .{ @as([]const u8, "System"), theme.warning },
        };
        const role_label = role_info[0];
        const role_color = role_info[1];
        const role_style = Style{ .fg = role_color, .bg = theme.background, .attr = .{ .bold = true } };
        _ = buf.writeStringBounded(x, current_y, role_label, role_style, width);
        current_y += 1;

        // Message parts
        for (msg.parts) |part| {
            switch (part) {
                .text => |t| {
                    const text_widget = TextWidget.init(t, Style{ .fg = theme.text, .bg = theme.background });
                    text_widget.render(buf, x + 1, current_y, width -| 2);
                    current_y += text_widget.lineCount(width -| 2);
                },
                .code => |c| {
                    // Code fence
                    const fence_style = Style{ .fg = theme.text_dim, .bg = theme.background_panel };
                    _ = buf.writeStringBounded(x + 1, current_y, "```", fence_style, width);
                    current_y += 1;

                    // Code content
                    var lines = std.mem.splitScalar(u8, c.code, '\n');
                    while (lines.next()) |line| {
                        const code_style = Style{ .fg = theme.syntax_string, .bg = theme.background_panel };
                    // Just render the line with background
                        for (0..@min(width - 2, @as(u16, @intCast(line.len + 4)))) |i| {
                            buf.setCell(x + 1 + @as(u16, @intCast(i)), current_y, .{ .style = .{ .bg = theme.background_panel } });
                        }
                        _ = buf.writeStringBounded(x + 3, current_y, line, code_style, width -| 4);
                        current_y += 1;
                    }

                    // Closing fence
                    _ = buf.writeStringBounded(x + 1, current_y, "```", fence_style, width);
                    current_y += 1;
                },
                .tool_use => |tu| {
                    const tool_style = Style{ .fg = theme.info, .bg = theme.background, .attr = .{ .italic = true } };
                    var name_buf: [128]u8 = undefined;
                    const label = std.fmt.bufPrint(&name_buf, "⚡ {s}", .{tu.name}) catch "⚡ tool";
                    _ = buf.writeStringBounded(x + 1, current_y, label, tool_style, width -| 2);
                    current_y += 1;
                },
                .tool_result => |tr| {
                    const result_style = Style{ .fg = theme.text_muted, .bg = theme.background };
                    var name_buf: [128]u8 = undefined;
                    const label = std.fmt.bufPrint(&name_buf, "  ↳ {s}", .{tr.name}) catch "  ↳ result";
                    _ = buf.writeStringBounded(x + 1, current_y, label, result_style, width -| 2);
                    current_y += 1;
                },
                .thinking => |t| {
                    if (self.show_thinking) {
                        const think_style = Style{ .fg = theme.text_dim, .bg = theme.background, .attr = .{ .italic = true } };
                        _ = buf.writeStringBounded(x + 1, current_y, "💭 ", think_style, width);
                        current_y += 1;
                        const think_widget = TextWidget.init(t, think_style);
                        think_widget.render(buf, x + 3, current_y, width -| 4);
                        current_y += think_widget.lineCount(width -| 4);
                    }
                },
                .err_text => |e| {
                    const err_style = Style{ .fg = theme.err_color, .bg = theme.background };
                    _ = buf.writeStringBounded(x + 1, current_y, "✗ ", err_style, width);
                    const err_widget = TextWidget.init(e, err_style);
                    err_widget.render(buf, x + 3, current_y, width -| 4);
                    current_y += err_widget.lineCount(width -| 4);
                },
            }
        }
        current_y += 1; // gap
    }

    fn renderPrompt(self: SessionView, buf: *Buffer, rect: Rect, theme: Theme) void {
        // Separator
        const sep_style = Style{ .fg = theme.border, .bg = theme.background };
        for (0..rect.width) |i| {
            buf.setCell(rect.x + @as(u16, @intCast(i)), rect.y, .{ .char = .{ .char = '─' }, .style = sep_style });
        }

        // Prompt border
        const border_style = Style{ .fg = theme.border, .bg = theme.background_panel };
        const input_y = rect.y + 1;
        const input_h = rect.height -| 1;

        // Top border
        buf.setCell(rect.x, input_y, .{ .char = .{ .char = '╭' }, .style = border_style });
        for (1..rect.width - 1) |i| {
            buf.setCell(rect.x + @as(u16, @intCast(i)), input_y, .{ .char = .{ .char = '─' }, .style = border_style });
        }
        buf.setCell(rect.x + rect.width - 1, input_y, .{ .char = .{ .char = '╮' }, .style = border_style });

        // Content
        for (1..input_h) |dy| {
            const row = input_y + @as(u16, @intCast(dy));
            buf.setCell(rect.x, row, .{ .char = .{ .char = '│' }, .style = border_style });
            for (1..rect.width - 1) |dx| {
                buf.setCell(rect.x + @as(u16, @intCast(dx)), row, .{ .style = .{ .bg = theme.background_panel } });
            }
            buf.setCell(rect.x + rect.width - 1, row, .{ .char = .{ .char = '│' }, .style = border_style });
        }

        // Bottom border
        const bottom_y = input_y + input_h - 1;
        buf.setCell(rect.x, bottom_y, .{ .char = .{ .char = '╰' }, .style = border_style });
        for (1..rect.width - 1) |i| {
            buf.setCell(rect.x + @as(u16, @intCast(i)), bottom_y, .{ .char = .{ .char = '─' }, .style = border_style });
        }
        buf.setCell(rect.x + rect.width - 1, bottom_y, .{ .char = .{ .char = '╯' }, .style = border_style });

        // Prompt text or placeholder
        const text_len = self.prompt.gap.length();
        const content_y = input_y + 1;
        if (text_len == 0) {
            const placeholder = "Type a message... (Enter to send, Shift+Tab for shell)";
            _ = buf.writeStringBounded(rect.x + 2, content_y, placeholder, Style{ .fg = theme.prompt_placeholder, .bg = theme.background_panel }, rect.width -| 4);
            buf.setCell(rect.x + 2, content_y, .{
                .char = .{ .char = '█' },
                .style = Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
            });
        } else {
            var text_buf: [2048]u8 = undefined;
            const text = self.prompt.gap.getText(&text_buf);
            _ = buf.writeStringBounded(rect.x + 2, content_y, text, Style{ .fg = theme.text, .bg = theme.background_panel }, rect.width -| 4);
        }
    }
};
