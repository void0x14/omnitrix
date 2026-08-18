const std = @import("std");
const term_mod = @import("core/terminal.zig");
const buffer_mod = @import("core/buffer.zig");
const theme_mod = @import("core/theme.zig");
const layout_mod = @import("core/layout.zig");
const home_mod = @import("views/home.zig");
const session_mod = @import("views/session.zig");
const sidebar_mod = @import("views/sidebar.zig");
const footer_mod = @import("views/footer.zig");
const modal_mod = @import("dialogs/modal.zig");
const palette_mod = @import("dialogs/command_palette.zig");
const Terminal = term_mod.Terminal;
const InputEvent = term_mod.InputEvent;
const KeyEvent = term_mod.KeyEvent;
const TerminalSize = term_mod.TerminalSize;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const ThemeMode = theme_mod.ThemeMode;
const HomeView = home_mod.HomeView;
const SessionView = session_mod.SessionView;
const ModalDialog = modal_mod.ModalDialog;
const CommandPalette = palette_mod.CommandPalette;
const Command = palette_mod.Command;

pub const ViewRoute = enum { home, session };

pub const App = struct {
    allocator: std.mem.Allocator,
    terminal: Terminal,
    theme: Theme,
    theme_mode: ThemeMode,
    route: ViewRoute,
    home: ?HomeView,
    session: ?SessionView,
    modal: ModalDialog,
    palette: CommandPalette,
    is_running: bool,
    frame_count: u64,
    tick_counter: u32,

    const commands = [_]Command{
        .{ .name = "session.new", .label = "New Session", .category = "Session", .shortcut = "Ctrl+N" },
        .{ .name = "session.list", .label = "Switch Session", .category = "Session", .shortcut = "Ctrl+L" },
        .{ .name = "session.rename", .label = "Rename Session", .category = "Session" },
        .{ .name = "session.share", .label = "Share Session", .category = "Session" },
        .{ .name = "session.undo", .label = "Undo Last Message", .category = "Session", .shortcut = "Ctrl+Z" },
        .{ .name = "model.list", .label = "Switch Model", .category = "Model", .shortcut = "Ctrl+M" },
        .{ .name = "model.cycle", .label = "Cycle Recent Model", .category = "Model" },
        .{ .name = "agent.list", .label = "Switch Agent", .category = "Agent" },
        .{ .name = "agent.cycle", .label = "Cycle Agent", .category = "Agent" },
        .{ .name = "theme.switch", .label = "Switch Theme", .category = "Appearance" },
        .{ .name = "theme.switch_mode", .label = "Toggle Dark/Light", .category = "Appearance" },
        .{ .name = "help.show", .label = "Show Help", .category = "General", .shortcut = "?" },
        .{ .name = "opencode.status", .label = "Show Status", .category = "General" },
        .{ .name = "opencode.debug", .label = "Debug Info", .category = "General" },
        .{ .name = "session.sidebar.toggle", .label = "Toggle Sidebar", .category = "View", .shortcut = "Ctrl+B" },
        .{ .name = "session.timeline", .label = "Jump to Message", .category = "Session", .shortcut = "Ctrl+T" },
    };

    pub fn init(allocator: std.mem.Allocator) !App {
        var terminal = try Terminal.init(allocator);
        terminal.enterAlternateScreen();
        terminal.enableMouseTracking();
        terminal.hideCursor();
        terminal.clearScreen();

        const theme_mode: ThemeMode = .dark;
        const theme = Theme.get(theme_mode);

        const palette = try CommandPalette.init(allocator, &commands, theme);

        return .{
            .allocator = allocator,
            .terminal = terminal,
            .theme = theme,
            .theme_mode = theme_mode,
            .route = .home,
            .home = null,
            .session = null,
            .modal = ModalDialog.init(theme),
            .palette = palette,
            .is_running = true,
            .frame_count = 0,
            .tick_counter = 0,
        };
    }

    pub fn deinit(self: *App) void {
        if (self.home) |*h| h.deinit();
        if (self.session) |*s| s.deinit();
        self.palette.deinit();
        self.terminal.deinit();
    }

    pub fn navigate(self: *App, route: ViewRoute) !void {
        self.route = route;
        switch (route) {
            .home => {
                if (self.session) |*s| {
                    s.deinit();
                    self.session = null;
                }
                self.home = try HomeView.init(self.allocator, self.theme);
            },
            .session => {
                if (self.home) |*h| {
                    h.deinit();
                    self.home = null;
                }
                self.session = try SessionView.init(self.allocator, self.theme);
                // Add demo messages
                try self.addDemoMessages();
            },
        }
    }

    fn addDemoMessages(self: *App) !void {
        if (self.session == null) return;
        const s = &self.session.?;

        try s.addMessage(.{
            .role = .user,
            .parts = &.{
                .{ .text = "Help me understand the Omnitrix architecture. What are the main modules and how do they interact?" },
            },
        });

        try s.addMessage(.{
            .role = .assistant,
            .parts = &.{
                .{ .text = "The Omnitrix is a Zig-based runtime system with the following core modules:" },
                .{ .text = "\n\n**Core Architecture:**\n\n" },
                .{ .text = "1. `omnitrix-io` - Platform event loop with epoll/kqueue/IOCP backends\n" },
                .{ .text = "2. `omnitrix-task` - Task lifecycle management with bounded active tasks\n" },
                .{ .text = "3. `omnitrix-net` - Provider network with SSE streaming and retry logic\n" },
                .{ .text = "4. `omnitrix-stream` - Unified stream model for events and markdown\n" },
                .{ .text = "5. `omnitrix-tui` - Terminal UI with double-buffered rendering\n" },
                .{ .text = "6. `omnitrix-ledger` - File mutation tracking and attribution\n" },
                .{ .text = "7. `omnitrix-permission` - Tool broker and capability rules\n" },
                .{ .text = "8. `omnitrix-runtime` - Agent state machine and scheduler\n" },
                .{ .text = "\nAll modules run in a **single process** with no IPC or JSON serialization between them." },
                .{ .text = "\n\n```zig\nconst runtime = @import(\"omnitrix-runtime\");\nconst tui = @import(\"omnitrix-tui\");\n\npub fn main() !void {\n    var app = try runtime.Engine.init(allocator);\n    defer app.deinit();\n    try app.run();\n}\n```" },
            },
        });

        try s.addMessage(.{
            .role = .user,
            .parts = &.{
                .{ .text = "How does the FileMutationLedger work? I need to understand the attribution system." },
            },
        });

        try s.addMessage(.{
            .role = .assistant,
            .parts = &.{
                .{ .text = "The FileMutationLedger tracks all file changes with **actor attribution**:" },
                .{ .text = "\n\nEach mutation record contains:\n- revision, path, old_path\n- before_hash and after_hash (BLAKE3)\n- kind (added/modified/deleted/renamed)\n- actor (user/agent/external)\n- agent_id, session_id, turn_id\n- hunk-level attribution when possible\n\nThe key insight is that **agent and user changes are tracked separately** at the hunk level. If two actors touch different hunks in the same file, each hunk retains its attribution. Only when attribution is unreliable does the file get marked `mixed_actor`." },
                .{ .tool_use = .{ .name = "read_file", .input = "zig/src/omnitrix-ledger/ledger.zig" } },
                .{ .tool_result = .{ .name = "read_file", .output = "File contents..." } },
            },
        });

        // Update sidebar info
        s.sidebar.info = .{
            .session_title = "Omnitrix Architecture Review",
            .session_id = "omni_01",
            .model_name = "MiMo-V2.5-Pro",
            .provider_name = "Codebuff",
            .branch_name = "masterplan",
            .version = "1.18.18",
            .files_changed = 12,
            .lines_added = 342,
            .lines_removed = 89,
        };

        // Update footer
        s.footer.info = .{
            .mode = "build",
            .model = "MiMo-V2.5-Pro",
            .tokens_prompt = 1247,
            .tokens_completion = 892,
            .is_streaming = false,
            .branch = "masterplan",
        };
    }

    /// Main event loop
    pub fn run(self: *App) !void {
        try self.navigate(.home);

        while (self.is_running) {
            // Check for terminal resize
            const new_size = try self.terminal.updateSize();
            _ = new_size;

            // Render current frame
            self.renderFrame() catch {};

            // Flush to terminal
            self.terminal.flush() catch {};

            // Read input with timeout
            const event = self.terminal.readEventTimeout(16); // ~60fps
            if (event) |evt| {
                try self.handleEvent(evt);
            }

            self.frame_count +%= 1;
            self.tick_counter +%= 1;

            // Update animations every 16 ticks (~256ms)
            if (self.tick_counter % 16 == 0) {
                if (self.session) |*s| {
                    if (s.messages.items.len > 0) {
                        const last = s.messages.items[s.messages.items.len - 1];
                        if (last.is_streaming) {
                            s.spinner.tick();
                        }
                    }
                }
            }
        }
    }

    fn renderFrame(self: *App) !void {
        self.terminal.buffer.clear();

        switch (self.route) {
            .home => {
                if (self.home) |*h| {
                    h.render(&self.terminal.buffer, self.terminal.size.cols, self.terminal.size.rows, self.theme);
                }
            },
            .session => {
                if (self.session) |*s| {
                    s.render(&self.terminal.buffer, self.terminal.size.cols, self.terminal.size.rows, self.theme);
                }
            },
        }

        // Render overlays on top
        self.modal.render(&self.terminal.buffer, self.terminal.size.cols, self.terminal.size.rows);
        self.palette.render(&self.terminal.buffer, self.terminal.size.cols, self.terminal.size.rows);
    }

    fn handleEvent(self: *App, event: InputEvent) !void {
        // Command palette has priority
        if (self.palette.visible) {
            const result = self.palette.handleKey(.{
                .char = if (event == .key) event.key.char else null,
                .enter = if (event == .key) event.key.key == .enter else false,
                .escape = if (event == .key) event.key.key == .escape else false,
                .up = if (event == .key) event.key.key == .up else false,
                .down = if (event == .key) event.key.key == .down else false,
                .backspace = if (event == .key) event.key.key == .backspace else false,
            });
            if (result) |cmd_name| {
                try self.executeCommand(cmd_name);
            }
            return;
        }

        // Modal dialog has priority
        if (self.modal.state.visible) {
            _ = self.modal.handleKey(.{
                .char = if (event == .key) event.key.char else null,
                .enter = if (event == .key) event.key.key == .enter else false,
                .escape = if (event == .key) event.key.key == .escape else false,
                .up = if (event == .key) event.key.key == .up else false,
                .down = if (event == .key) event.key.key == .down else false,
            });
            return;
        }

        switch (event) {
            .resize => |size| {
                self.terminal.size = size;
                try self.terminal.buffer.resize(size.cols, size.rows);
            },
            .key => |key| {
                try self.handleKey(key);
            },
            .mouse => |mouse| {
                try self.handleMouse(mouse);
            },
        }
    }

    fn handleKey(self: *App, key: KeyEvent) !void {
        // Global keybindings
        if (key.ctrl) {
            switch (key.char orelse 0) {
                'c' => {
                    self.is_running = false;
                    return;
                },
                'p' => {
                    self.palette.toggle();
                    return;
                },
                'n' => {
                    try self.navigate(.session);
                    return;
                },
                'b' => {
                    if (self.session) |_| {
                        self.session.?.toggleSidebar();
                    }
                    return;
                },
                'l' => {
                    self.modal.showSelect("Switch Session", &.{ "Session 1: Omnitrix Architecture", "Session 2: TUI Implementation", "Session 3: Bug Fix #42" });
                    return;
                },
                'm' => {
                    self.modal.showSelect("Switch Model", &.{ "MiMo-V2.5-Pro", "Claude Sonnet 4", "GPT-4o", "Gemini 2.5 Pro", "DeepSeek V3" });
                    return;
                },
                't' => {
                    if (self.session != null) {
                        self.modal.showSelect("Jump to Message", &.{ "Message 1: Help me understand...", "Message 2: The Omnitrix is...", "Message 3: How does the FileMutationLedger...", "Message 4: The FileMutationLedger tracks..." });
                    }
                    return;
                },
                'z' => {
                    if (self.route == .session) {
                        self.modal.showAlert("Undo", "Last message reverted.");
                    }
                    return;
                },
                else => {},
            }
        }

        if (key.key == .escape) {
            if (self.route == .home and self.home != null) {
                if (self.home.?.prompt.gap.length() > 0) {
                    self.home.?.prompt.gap.clear();
                    return;
                }
            }
            self.is_running = false;
            return;
        }

        switch (self.route) {
            .home => {
                try self.handleHomeKey(key);
            },
            .session => {
                try self.handleSessionKey(key);
            },
        }
    }

    fn handleHomeKey(self: *App, key: KeyEvent) !void {
        const h = &(self.home orelse return);

        if (key.key == .enter) {
            if (!h.prompt.isEmpty()) {
                // Navigate to session and add the user message
                try self.navigate(.session);
                if (self.session) |*s| {
                    const text = try h.prompt.getText();
                    defer self.allocator.free(text);
                    try s.addMessage(.{
                        .role = .user,
                        .parts = &.{.{ .text = text }},
                    });
                }
            }
            return;
        }

        try h.prompt.handleKey(.{
            .char = key.char,
            .key = switch (key.key) {
                .enter => .enter,
                .backspace => .backspace,
                .delete => .delete,
                .left => .left,
                .right => .right,
                .up => .up,
                .down => .down,
                .home => .home,
                .end => .end,
                else => .none,
            },
            .ctrl = key.ctrl,
            .alt = key.alt,
        });
    }

    fn handleSessionKey(self: *App, key: KeyEvent) !void {
        const s = &(self.session orelse return);

        if (key.key == .enter and !key.shift) {
            if (!s.prompt.isEmpty()) {
                const text = try s.prompt.getText();
                defer self.allocator.free(text);
                try s.addMessage(.{
                    .role = .user,
                    .parts = &.{.{ .text = text }},
                });
                s.prompt.gap.clear();
                s.scroll.scrollToBottom();
            }
            return;
        }

        if (key.key == .page_up) {
            s.scroll.pageUp();
            return;
        }
        if (key.key == .page_down) {
            s.scroll.pageDown();
            return;
        }

        // Forward to prompt
        try s.prompt.handleKey(.{
            .char = key.char,
            .key = switch (key.key) {
                .enter => .enter,
                .backspace => .backspace,
                .delete => .delete,
                .left => .left,
                .right => .right,
                .up => .up,
                .down => .down,
                .home => .home,
                .end => .end,
                else => .none,
            },
            .ctrl = key.ctrl,
            .alt = key.alt,
        });
    }

    fn handleMouse(self: *App, mouse: term_mod.MouseEvent) !void {
        switch (self.route) {
            .session => {
                if (self.session) |*s| {
                    if (mouse.btn == .scroll_up) {
                        s.scroll.scrollUp();
                    } else if (mouse.btn == .scroll_down) {
                        s.scroll.scrollDown();
                    }
                }
            },
            else => {},
        }
    }

    fn executeCommand(self: *App, cmd_name: []const u8) !void {
        if (std.mem.eql(u8, cmd_name, "session.new")) {
            try self.navigate(.session);
        } else if (std.mem.eql(u8, cmd_name, "session.list")) {
            self.modal.showSelect("Switch Session", &.{ "Session 1", "Session 2", "Session 3" });
        } else if (std.mem.eql(u8, cmd_name, "model.list")) {
            self.modal.showSelect("Switch Model", &.{ "MiMo-V2.5-Pro", "Claude Sonnet 4", "GPT-4o" });
        } else if (std.mem.eql(u8, cmd_name, "theme.switch_mode")) {
            self.theme_mode = switch (self.theme_mode) {
                .dark => .light,
                .light => .dark,
            };
            self.theme = Theme.get(self.theme_mode);
            self.modal = ModalDialog.init(self.theme);
        } else if (std.mem.eql(u8, cmd_name, "help.show")) {
            self.modal.showAlert("Help", "Ctrl+P: Command palette\nCtrl+N: New session\nCtrl+B: Toggle sidebar\nCtrl+M: Switch model\nCtrl+L: Switch session\nCtrl+Z: Undo\nEsc: Exit");
        } else if (std.mem.eql(u8, cmd_name, "session.sidebar.toggle")) {
            if (self.session) |*s| {
                s.toggleSidebar();
            }
        } else if (std.mem.eql(u8, cmd_name, "opencode.status")) {
            self.modal.showAlert("Status", "Omnitrix TUI v1.18.18\nZig Runtime\nAll systems operational");
        }
    }
};
