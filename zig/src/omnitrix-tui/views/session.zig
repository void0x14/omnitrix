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
const conversation_mod = @import("conversation.zig");
const changes_mod = @import("changes.zig");
const diff_mod = @import("diff.zig");
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
const BlockStore = conversation_mod.BlockStore;
const SidebarInfo = sidebar_mod.SidebarInfo;
const FooterView = footer_mod.FooterView;
const FooterInfo = footer_mod.FooterInfo;

pub const SessionAction = union(enum) {
    none,
    send,
    cancel,
    focus_prompt,
    focus_conversation,
    toggle_block: u32,
};

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
    stream_committed_len: usize = 0,
    id: []const u8 = "",
};

pub const SidebarTab = enum { conversation, changes, diff };

const max_message_count: usize = 512;
const max_message_bytes: usize = 256 * 1024;
const max_message_parts: usize = 32;
const max_message_field_bytes: usize = 1024;

pub const SessionView = struct {
    allocator: std.mem.Allocator,
    /// Owns every string reachable from `messages`. Callers of `addMessage`
    /// may pass stack temporaries or buffers they free immediately; the view
    /// deep-copies into this arena so rendering never touches freed memory.
    content_arena: std.heap.ArenaAllocator,
    messages: std.ArrayList(Message),
    message_bytes: usize,
    blocks: BlockStore,
    next_block_id: u32,
    last_streaming_id: ?u32,
    scroll: ScrollContainer,
    prompt: TextareaWidget,
    markdown: MarkdownRenderer,
    spinner: Spinner,
    sidebar: SidebarView,
    footer: FooterView,
    sidebar_visible: bool,
    focused: enum { conversation, prompt },
    sidebar_tab: SidebarTab,
    selected_file: u16,
    show_timestamps: bool,
    show_thinking: bool,
    show_tool_details: bool,

    pub const SidebarView = sidebar_mod.SidebarView;

    pub fn init(allocator: std.mem.Allocator, theme: Theme) !SessionView {
        return .{
            .allocator = allocator,
            .content_arena = std.heap.ArenaAllocator.init(allocator),
            .messages = .empty,
            .message_bytes = 0,
            .blocks = BlockStore.init(allocator),
            .next_block_id = 1,
            .last_streaming_id = null,
            .scroll = ScrollContainer.init(30, 80),
            .prompt = try TextareaWidget.init(
                allocator,
                Style{ .fg = theme.text, .bg = theme.background_panel },
                Style{ .fg = theme.prompt_cursor, .bg = theme.background_panel },
            ),
            .markdown = MarkdownRenderer.init(theme),
            .spinner = Spinner.init("Thinking...", Style{ .fg = theme.accent }, Style{ .fg = theme.text_muted }),
            .sidebar = SidebarView.init(allocator),
            .footer = FooterView.init(),
            .sidebar_visible = true,
            .focused = .prompt,
            .sidebar_tab = .conversation,
            .selected_file = 0,
            .show_timestamps = false,
            .show_thinking = true,
            .show_tool_details = true,
        };
    }

    pub fn deinit(self: *SessionView) void {
        self.messages.deinit(self.allocator);
        self.blocks.deinit();
        self.content_arena.deinit();
        self.prompt.deinit();
        self.sidebar.deinit();
    }

    /// Append a message, deep-copying its payload into the view's arena.
    ///
    /// The caller keeps ownership of whatever it passed in and is free to
    /// release it as soon as this returns.
    pub fn addMessage(self: *SessionView, msg: Message) !void {
        self.last_streaming_id = null;
        const estimated = @min(estimateMessageBytes(msg), max_message_bytes);
        if (self.messages.items.len >= max_message_count or
            self.message_bytes > max_message_bytes -| estimated)
        {
            self.resetMessageRetention();
        }

        const arena = self.content_arena.allocator();
        const part_count = @min(msg.parts.len, max_message_parts);
        const parts = try arena.alloc(MessagePart, part_count);
        for (msg.parts[0..part_count], 0..) |part, i| {
            parts[i] = try dupePart(arena, part);
        }

        var owned = msg;
        owned.parts = parts;
        owned.id = try arena.dupe(u8, boundedText(msg.id, max_message_field_bytes));
        try self.messages.append(self.allocator, owned);
        self.message_bytes +|= estimated;

        for (msg.parts[0..part_count]) |part| {
            switch (part) {
                .text => |text| try self.appendConversationBlock(.text, text, msg.is_streaming, msg.stream_committed_len),
                .thinking => |text| try self.appendConversationBlock(.thinking, text, msg.is_streaming, msg.stream_committed_len),
                .err_text => |text| try self.appendConversationBlock(.@"error", text, msg.is_streaming, msg.stream_committed_len),
                .code => |code| try self.appendConversationBlock(.code, code.code, msg.is_streaming, msg.stream_committed_len),
                .tool_use => |tool| try self.appendConversationBlock(.tool_use, tool.input, msg.is_streaming, msg.stream_committed_len),
                .tool_result => |result| try self.appendConversationBlock(.tool_result, result.output, msg.is_streaming, msg.stream_committed_len),
            }
        }
        self.scrollToBottom();
    }

    fn resetMessageRetention(self: *SessionView) void {
        self.messages.clearRetainingCapacity();
        _ = self.content_arena.reset(.retain_capacity);
        self.message_bytes = 0;
    }

    fn boundedText(text: []const u8, cap: usize) []const u8 {
        var end = @min(text.len, cap);
        while (end > 0 and !std.unicode.utf8ValidateSlice(text[0..end])) end -= 1;
        return text[0..end];
    }

    fn messagePartBytes(part: MessagePart) usize {
        return switch (part) {
            .text => |text| boundedText(text, max_message_field_bytes).len,
            .thinking => |text| boundedText(text, max_message_field_bytes).len,
            .err_text => |text| boundedText(text, max_message_field_bytes).len,
            .code => |code| boundedText(code.lang, max_message_field_bytes).len + boundedText(code.code, max_message_field_bytes).len,
            .tool_use => |tool| boundedText(tool.name, max_message_field_bytes).len + boundedText(tool.input, max_message_field_bytes).len,
            .tool_result => |result| boundedText(result.name, max_message_field_bytes).len + boundedText(result.output, max_message_field_bytes).len,
        };
    }

    fn estimateMessageBytes(msg: Message) usize {
        const count = @min(msg.parts.len, max_message_parts);
        var total: usize = @sizeOf(Message) +| (@sizeOf(MessagePart) *| count);
        total +|= boundedText(msg.id, max_message_field_bytes).len;
        for (msg.parts[0..count]) |part| total +|= messagePartBytes(part);
        return total;
    }

    fn initialStreamingPrefix(text: []const u8, requested: usize) usize {
        const target = if (requested > 0) requested else text.len / 2;
        return conversation_mod.BlockStore.safePrefixLen(text, target);
    }

    fn appendConversationBlock(self: *SessionView, kind: conversation_mod.BlockKind, text: []const u8, streaming: bool, committed_len: usize) !void {
        const id = self.next_block_id;
        self.next_block_id +%= 1;
        try self.blocks.append(.{
            .id = id,
            .kind = kind,
            .text = text,
            .committed_len = if (streaming) initialStreamingPrefix(text, committed_len) else text.len,
            .is_streaming = streaming,
        });
        if (streaming) self.last_streaming_id = id;
    }

    /// Append a progressive UI chunk into the BlockStore-owned streaming text.
    pub fn appendStreamingChunk(self: *SessionView, id: u32, chunk: []const u8) !bool {
        const updated = try self.blocks.appendStreamingChunk(id, chunk);
        if (updated) self.scrollToBottom();
        return updated;
    }

    /// Returns the most recently created streaming block, if any.
    pub fn lastStreamingBlockId(self: *const SessionView) ?u32 {
        return self.last_streaming_id;
    }

    pub fn commitStreaming(self: *SessionView, id: u32, committed_len: usize) bool {
        return self.blocks.setCommittedPrefix(id, committed_len);
    }

    pub fn finishStreaming(self: *SessionView, id: u32) bool {
        return self.blocks.finishStreaming(id);
    }

    fn dupePart(arena: std.mem.Allocator, part: MessagePart) !MessagePart {
        return switch (part) {
            .text => |t| .{ .text = try arena.dupe(u8, boundedText(t, max_message_field_bytes)) },
            .thinking => |t| .{ .thinking = try arena.dupe(u8, boundedText(t, max_message_field_bytes)) },
            .err_text => |t| .{ .err_text = try arena.dupe(u8, boundedText(t, max_message_field_bytes)) },
            .code => |c| .{ .code = .{
                .lang = try arena.dupe(u8, boundedText(c.lang, max_message_field_bytes)),
                .code = try arena.dupe(u8, boundedText(c.code, max_message_field_bytes)),
            } },
            .tool_use => |c| .{ .tool_use = .{
                .name = try arena.dupe(u8, boundedText(c.name, max_message_field_bytes)),
                .input = try arena.dupe(u8, boundedText(c.input, max_message_field_bytes)),
            } },
            .tool_result => |c| .{ .tool_result = .{
                .name = try arena.dupe(u8, boundedText(c.name, max_message_field_bytes)),
                .output = try arena.dupe(u8, boundedText(c.output, max_message_field_bytes)),
            } },
        };
    }

    pub fn scrollToBottom(self: *SessionView) void {
        self.scroll.scrollToBottom();
    }

    pub fn toggleSidebar(self: *SessionView) void {
        self.sidebar_visible = !self.sidebar_visible;
    }

    pub fn cycleSidebarTab(self: *SessionView, direction: i8) void {
        const new: i8 = @as(i8, @intFromEnum(self.sidebar_tab)) + direction;
        self.sidebar_tab = switch (@mod(new, 3)) {
            0 => .conversation,
            1 => .changes,
            2 => .diff,
            else => unreachable,
        };
    }

    /// Handle a mouse click on the header row; returns true if a tab was hit.
    pub fn handleHeaderClick(self: *SessionView, click_x: u16, content_width: u16) bool {
        // Tabs are rendered right-to-left in the header at y=0
        // [Conversation] [Changes] [Diff]
        const tabs = [_]struct { label: []const u8, tab: SidebarTab }{
            .{ .label = "Conversation", .tab = .conversation },
            .{ .label = "Changes", .tab = .changes },
            .{ .label = "Diff", .tab = .diff },
        };

        // Calculate tab positions (same logic as renderHeader)
        var tab_x: u16 = content_width -| 2;
        var positions: [3]u16 = [_]u16{ 0, 0, 0 };
        var widths: [3]u16 = [_]u16{ 0, 0, 0 };
        var i: usize = tabs.len;
        while (i > 0) {
            i -= 1;
            const label_w: u16 = @intCast(tabs[i].label.len + 2);
            if (tab_x >= label_w + 1) {
                tab_x -= label_w + 1;
                positions[i] = tab_x;
                widths[i] = label_w;
            }
        }

        // Check which tab was clicked
        for (tabs, 0..) |t, idx| {
            if (widths[idx] == 0) continue;
            if (click_x >= positions[idx] and click_x < positions[idx] + widths[idx]) {
                self.sidebar_tab = t.tab;
                return true;
            }
        }
        return false;
    }

    /// Render the entire session view
    pub fn render(self: *SessionView, buf: *Buffer, terminal_width: u16, terminal_height: u16, theme: Theme) void {
        // Clear
        buf.fillRegion(0, 0, terminal_width, terminal_height, .{ .style = .{ .bg = theme.background } });

        const diff_fullscreen = self.sidebar_tab == .diff and terminal_width < self.sidebar.width +| 62;
        const show_sidebar = self.sidebar_visible and !diff_fullscreen;
        const sidebar_w: u16 = if (show_sidebar) self.sidebar.width +| 2 else 0;
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
        switch (self.sidebar_tab) {
            .conversation => self.renderConversation(buf, conv_rect, theme),
            .changes => changes_mod.render(buf, conv_rect, self.sidebar.info, self.selected_file, theme),
            .diff => diff_mod.render(buf, conv_rect, self.sidebar.info, .{
                .selected_file = self.selected_file,
                .selected_hunk = 0,
                .fullscreen = diff_fullscreen,
                .unavailable = true,
            }, theme),
        }

        // Prompt area
        const prompt_rect = Rect{
            .x = 0,
            .y = header_y + 1 + conversation_height,
            .width = content_width,
            .height = prompt_height,
        };
        self.renderPrompt(buf, prompt_rect, theme);

        // Sidebar
        if (show_sidebar) {
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

    fn renderHeader(self: SessionView, buf: *Buffer, x: u16, y: u16, width: u16, theme: Theme) void {
        // Header background
        const header_style = Style{ .fg = theme.text, .bg = theme.background_panel, .attr = .{ .bold = true } };
        for (0..width) |i| {
            buf.setCell(x + @as(u16, @intCast(i)), y, .{ .style = header_style });
        }
        _ = buf.writeStringBounded(x + 2, y, "Omnitrix", header_style, width -| 4);

        // Right side: tab indicators (interactive)
        const tabs = [_]struct { label: []const u8, tab: SidebarTab }{
            .{ .label = "Conversation", .tab = .conversation },
            .{ .label = "Changes", .tab = .changes },
            .{ .label = "Diff", .tab = .diff },
        };

        var tab_x: u16 = x + width -| 2;
        // Render tabs right-to-left to calculate positions, then draw
        var tab_positions: [3]struct { label: []const u8, start_x: u16, width: u16 } = undefined;
        var i: usize = tabs.len;
        while (i > 0) {
            i -= 1;
            const t = tabs[i];
            const label_w: u16 = @intCast(t.label.len + 2); // +2 for brackets
            if (tab_x >= label_w + 1) {
                tab_x -= label_w + 1; // +1 for space between tabs
                tab_positions[i] = .{ .label = t.label, .start_x = tab_x, .width = label_w };
            } else {
                tab_positions[i] = .{ .label = t.label, .start_x = 0, .width = 0 };
            }
        }

        // Draw tabs
        for (tabs, 0..) |t, idx| {
            const pos = tab_positions[idx];
            if (pos.width == 0) continue;

            const is_active = self.sidebar_tab == t.tab;
            const tab_style = if (is_active)
                Style{ .fg = theme.accent, .bg = theme.background_panel, .attr = .{ .bold = true } }
            else
                Style{ .fg = theme.text_muted, .bg = theme.background_panel };

            // Draw "[label]"
            buf.setCell(pos.start_x, y, .{ .char = .{ .char = '[' }, .style = tab_style });
            _ = buf.writeStringBounded(pos.start_x + 1, y, t.label, tab_style, @intCast(t.label.len));
            buf.setCell(pos.start_x + 1 + @as(u16, @intCast(t.label.len)), y, .{ .char = .{ .char = ']' }, .style = tab_style });

            // Underline active tab
            if (is_active) {
                for (0..pos.width) |wi| {
                    var cell = buf.getCell(pos.start_x + @as(u16, @intCast(wi)), y + 1);
                    cell.style.attr.underline = true;
                    cell.style.fg = theme.accent;
                    buf.setCell(pos.start_x + @as(u16, @intCast(wi)), y + 1, cell);
                }
            }
        }
    }

    fn renderConversation(self: *SessionView, buf: *Buffer, rect: Rect, theme: Theme) void {
        conversation_mod.render(&self.blocks, self.markdown, &self.spinner, &self.scroll, buf, rect, theme);
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
            const placeholder = "Type a message... (Enter to send, Tab for shell)";
            _ = buf.writeStringBounded(rect.x + 2, content_y, placeholder, Style{ .fg = theme.prompt_placeholder, .bg = theme.background_panel }, rect.width -| 4);
            buf.setCell(rect.x + 1, content_y, .{
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
