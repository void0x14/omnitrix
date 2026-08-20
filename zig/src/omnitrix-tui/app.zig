const std = @import("std");
const term_mod = @import("core/terminal.zig");
const buffer_mod = @import("core/buffer.zig");
const theme_mod = @import("core/theme.zig");
const layout_mod = @import("core/layout.zig");
const home_mod = @import("views/home.zig");
const welcome_mod = @import("views/welcome.zig");
const session_mod = @import("views/session.zig");
const sidebar_mod = @import("views/sidebar.zig");
const footer_mod = @import("views/footer.zig");
const modal_mod = @import("dialogs/modal.zig");
const palette_mod = @import("dialogs/command_palette.zig");
const ui_state_mod = @import("core/ui_state.zig");
const Terminal = term_mod.Terminal;
const InputEvent = term_mod.InputEvent;
const KeyEvent = term_mod.KeyEvent;
const TerminalSize = term_mod.TerminalSize;
const Buffer = buffer_mod.Buffer;
const Theme = theme_mod.Theme;
const ThemeMode = theme_mod.ThemeMode;
const HomeView = home_mod.HomeView;
const WelcomeView = welcome_mod.WelcomeView;
const WelcomeAction = welcome_mod.WelcomeAction;
const AuthMethod = welcome_mod.AuthMethod;
const SessionView = session_mod.SessionView;
const ModalDialog = modal_mod.ModalDialog;
const CommandPalette = palette_mod.CommandPalette;
const Command = palette_mod.Command;
const UiState = ui_state_mod.UiState;
const Viewport = ui_state_mod.Viewport;

pub const ViewRoute = ui_state_mod.Route;

pub const App = struct {
    allocator: std.mem.Allocator,
    terminal: Terminal,
    theme: Theme,
    theme_mode: ThemeMode,
    ui: UiState,
    welcome: WelcomeView,
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
        errdefer terminal.deinit();
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
            .ui = .{ .viewport = .{ .cols = terminal.size.cols, .rows = terminal.size.rows } },
            .welcome = WelcomeView.init(),
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
        self.welcome.deinit();
        if (self.home) |*h| h.deinit();
        if (self.session) |*s| s.deinit();
        self.palette.deinit();
        self.terminal.deinit();
    }

    pub fn navigate(self: *App, route: ViewRoute) !void {
        self.ui.route = route;
        switch (route) {
            .home => {
                if (self.home) |*h| h.deinit();
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
            .welcome, .too_small => {},
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
            .files_changed = 6,
            .lines_added = 342,
            .lines_removed = 89,
            .changed_files = &.{
                .{ .path = "zig/src/omnitrix-tui/app.zig", .added = 120, .removed = 40 },
                .{ .path = "zig/src/omnitrix-tui/core/terminal.zig", .added = 80, .removed = 20 },
                .{ .path = "zig/src/omnitrix-tui/core/buffer.zig", .added = 45, .removed = 12 },
                .{ .path = "zig/src/omnitrix-tui/views/home.zig", .added = 30, .removed = 8 },
                .{ .path = "zig/src/omnitrix-tui/views/sidebar.zig", .added = 25, .removed = 5 },
                .{ .path = "zig/build.zig", .added = 42, .removed = 4 },
            },
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
        while (self.is_running) {
            // Check for terminal resize
            const new_size = try self.terminal.updateSize();
            self.ui.setViewport(new_size.cols, new_size.rows);

            // Render current frame
            self.renderFrame() catch {};

            // Flush to terminal
            self.terminal.flush() catch {};

            // Render Kitty graphics overlay (if supported)
            self.renderKittyOverlay();

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

    /// Render Kitty graphics overlay (Omnitrix logo).
    /// Emits the Kitty image only when the terminal actually supports the
    /// protocol; everywhere else the sidebar already draws a Unicode dial,
    /// and sending the raw payload would paint base64 garbage over the UI.
    fn renderKittyOverlay(self: *App) void {
        if (!self.terminal.kitty_graphics) return;
        if (self.ui.route != .session or self.ui.viewport.isTooSmall()) return;
        if (self.session) |*s| {
            if (!s.sidebar_visible) return;

            // Logo position: just inside the sidebar's right edge, bottom.
            const sidebar_w: u16 = s.sidebar.width + 2;
            const content_width = self.terminal.size.cols -| sidebar_w;
            const logo_x = content_width + 2;
            const logo_y = self.terminal.size.rows -| 2;

            // Mutate the real sidebar through the pointer captured above.
            // Copying the session struct here used to allocate the command
            // into a throwaway copy every frame (~1.5KB leaked per frame).
            s.sidebar.ensureKittyLogo(logo_x, logo_y);

            if (s.sidebar.kitty_logo_cmd) |cmd| {
                self.terminal.renderKittyAt(logo_x, logo_y, cmd);
            }
        }
    }

    fn renderFrame(self: *App) !void {
        self.terminal.buffer.clear();

        if (self.ui.viewport.isTooSmall()) {
            self.renderTooSmall();
            return;
        }

        switch (self.ui.route) {
            .home => {
                if (self.home) |*h| {
                    h.render(&self.terminal.buffer, self.ui.viewport.cols, self.ui.viewport.rows, self.theme);
                }
            },
            .welcome => self.welcome.render(&self.terminal.buffer, self.ui.viewport.cols, self.ui.viewport.rows, self.theme),
            .session => {
                if (self.session) |*s| {
                    const frame = layout_mod.sessionLayout(
                        self.ui.viewport.cols,
                        self.ui.viewport.rows,
                        s.sidebar_visible,
                        s.sidebar.width,
                    );
                    s.scroll.setViewport(frame.conversation.height, frame.conversation.width);
                    s.render(&self.terminal.buffer, self.ui.viewport.cols, self.ui.viewport.rows, self.theme);
                }
            },
            .too_small => self.renderTooSmall(),
        }

        // Render order is deterministic: question/confirm, select/alert,
        // palette, then page. ModalDialog is mutually exclusive by type.
        self.modal.render(&self.terminal.buffer, self.ui.viewport.cols, self.ui.viewport.rows);
        self.palette.render(&self.terminal.buffer, self.ui.viewport.cols, self.ui.viewport.rows);
    }

    fn renderTooSmall(self: *App) void {
        const cols = self.ui.viewport.cols;
        const rows = self.ui.viewport.rows;
        if (cols == 0 or rows == 0) return;

        const bg = self.theme.background;
        self.terminal.buffer.fillRegion(0, 0, cols, rows, .{ .style = .{ .bg = bg } });

        const title = "Terminal too small";
        const required = "Resize to at least 80 columns x 24 rows";
        const title_width: u16 = @intCast(title.len);
        const required_width: u16 = @intCast(required.len);
        const title_x = if (cols > title_width) (cols - title_width) / 2 else 0;
        const required_x = if (cols > required_width) (cols - required_width) / 2 else 0;
        const y = rows / 2;
        _ = self.terminal.buffer.writeStringBounded(title_x, y, title, .{
            .fg = self.theme.warning,
            .bg = bg,
            .attr = .{ .bold = true },
        }, cols -| title_x);
        if (y + 1 < rows) {
            _ = self.terminal.buffer.writeStringBounded(required_x, y + 1, required, .{
                .fg = self.theme.text_muted,
                .bg = bg,
            }, cols -| required_x);
        }
    }

    fn handleEvent(self: *App, event: InputEvent) !void {
        // Ctrl+C must always quit, before any overlay gets a chance to consume
        // it. The terminal runs with ISIG disabled, so the kernel never turns
        // this into SIGINT -- if the app swallows it too, the only way out is
        // SIGKILL from another shell.
        if (event == .key and event.key.ctrl and (event.key.char orelse 0) == 'c') {
            self.is_running = false;
            return;
        }

        // Overlay input order is deterministic: question/confirm, select/alert,
        // palette, then page. ModalDialog is mutually exclusive by type.
        if (self.modal.state.visible) {
            const was_visible = self.modal.state.visible;
            _ = self.modal.handleKey(.{
                .char = if (event == .key) event.key.char else null,
                .enter = if (event == .key) event.key.key == .enter else false,
                .escape = if (event == .key) event.key.key == .escape else false,
                .up = if (event == .key) event.key.key == .up else false,
                .down = if (event == .key) event.key.key == .down else false,
            });
            if (was_visible and !self.modal.state.visible and self.modal.state.confirmed) {
                if (self.modal.selectedValue()) |val| {
                    const title = self.modal.state.title[0..self.modal.state.title_len];
                    if (std.mem.eql(u8, title, "Switch Model")) {
                        if (self.session) |*s| s.footer.info.model = val;
                    } else if (std.mem.eql(u8, title, "Switch Agent")) {
                        if (self.session) |*s| s.footer.info.mode = val;
                    }
                }
            }
            return;
        }

        if (self.palette.visible) {
            const result = self.palette.handleKey(.{
                .char = if (event == .key) event.key.char else null,
                .enter = if (event == .key) event.key.key == .enter else false,
                .escape = if (event == .key) event.key.key == .escape else false,
                .up = if (event == .key) event.key.key == .up else false,
                .down = if (event == .key) event.key.key == .down else false,
                .backspace = if (event == .key) event.key.key == .backspace else false,
            });
            if (result) |cmd_name| try self.executeCommand(cmd_name);
            return;
        }

        switch (event) {
            .resize => |size| {
                self.terminal.size = size;
                try self.terminal.buffer.resize(size.cols, size.rows);
                self.ui.setViewport(size.cols, size.rows);
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
        if (self.ui.route == .welcome) {
            try self.handleWelcomeKey(key);
            return;
        }

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
                    if (self.ui.route == .session) {
                        self.modal.showAlert("Undo", "Last message reverted.");
                    }
                    return;
                },
                else => {},
            }
        }

        if (key.key == .escape) {
            if (self.ui.route == .home and self.home != null) {
                if (self.home.?.composer_mode != .normal or self.home.?.notice_len > 0) {
                    self.home.?.setComposerMode(.normal);
                    self.home.?.notice_len = 0;
                    return;
                }
                if (self.home.?.prompt.gap.length() > 0) {
                    self.home.?.prompt.gap.clear();
                    return;
                }
                self.is_running = false;
                return;
            }
            // On the session route Esc is a no-op: overlays (palette,
            // modals) consume their own Esc first, and a stray Esc must
            // never kill an active session. Ctrl+C remains the explicit
            // exit. Quitting only happens from home with an empty prompt.
            return;
        }

        switch (self.ui.route) {
            .welcome => {},
            .home => {
                try self.handleHomeKey(key);
            },
            .session => {
                try self.handleSessionKey(key);
            },
            .too_small => {},
        }
    }

    fn handleWelcomeKey(self: *App, key: KeyEvent) !void {
        const action = self.welcome.handleKey(key);
        switch (action) {
            .none => {},
            .choose_method => |method| {
                self.welcome.setState(.choosing, chooseNotice(method));
            },
            .begin_auth => |method| {
                if (method == .api_key) {
                    self.welcome.setState(.failed, "API-key sign-in is visual-only in this UI fixture. Press R to retry.");
                } else {
                    self.welcome.setState(.pending, pendingNotice(method));
                }
            },
            .cancel => {
                self.welcome.setState(.signed_out, "Sign-in canceled. Press Enter to choose a method again.");
            },
            .complete => {
                self.welcome.setState(.authenticated, "Fixture sign-in complete. Opening home.");
                try self.navigate(.home);
            },
            .retry => {
                self.welcome.setState(.choosing, "Choose a fixture method to retry.");
            },
        }
    }

    fn chooseNotice(method: AuthMethod) []const u8 {
        return switch (method) {
            .browser => "Browser sign-in selected. Press Enter to begin the fixture.",
            .device_code => "Device-code handoff selected. Press Enter to begin the fixture.",
            .api_key => "API-key row is masked and visual-only. Press Enter to show its fixture failure.",
        };
    }

    fn pendingNotice(method: AuthMethod) []const u8 {
        return switch (method) {
            .browser => "Browser handoff pending. Enter completes the fixture; F shows failure.",
            .device_code => "Device-code handoff pending. Enter completes the fixture; F shows failure.",
            .api_key => "API-key fixture pending.",
        };
    }

    fn handleHomeKey(self: *App, key: KeyEvent) !void {
        const h = &(self.home orelse return);

        if (key.ctrl and (key.char orelse 0) == 'k') {
            h.prompt.gap.clear();
            h.setComposerMode(.normal);
            h.setNotice("Composer cleared.");
            return;
        }

        if (key.ctrl and (key.char orelse 0) == 'r') {
            h.setComposerMode(.history);
            h.setNotice("History is a UI fixture; no persisted entries are loaded.");
            return;
        }

        if (key.key == .tab) {
            h.setComposerMode(if (h.composer_mode == .shell) .normal else .shell);
            h.setNotice(if (h.composer_mode == .shell) "Shell fixture selected; no command will execute." else "Normal composer mode.");
            return;
        }

        if (key.key == .up and h.prompt.isEmpty()) {
            h.setComposerMode(.history);
            h.setNotice("History is a UI fixture; no persisted entries are loaded.");
            return;
        }

        if (key.key == .enter and key.shift) {
            h.setComposerMode(.multiline);
            if (h.prompt.gap.length() < HomeView.max_prompt_bytes) {
                try h.prompt.handleKey(.{ .key = .enter });
            }
            return;
        }

        if (key.key == .enter) {
            if (h.composer_mode == .history) {
                h.setComposerMode(.normal);
                h.setNotice("History closed.");
                return;
            }
            if (h.composer_mode == .shell) {
                h.setNotice("Shell fixture selected; execution is unavailable in the UI-only pass.");
                return;
            }
            if (!h.prompt.isEmpty()) {
                // The text MUST be copied out before navigating: navigate(.session)
                // calls home.deinit(), which frees the prompt's gap buffer. Reading
                // `h.prompt` afterwards is a use-after-free and faults inside the
                // allocator vtable.
                const text = try h.prompt.getText();
                defer self.allocator.free(text);

                try self.navigate(.session);
                if (self.session) |*s| {
                    try s.addMessage(.{
                        .role = .user,
                        .parts = &.{.{ .text = text }},
                    });
                }
            } else {
                h.setComposerMode(.@"error");
                h.setNotice("Nothing to send. Type a prompt or choose a composer mode.");
            }
            return;
        }

        if (h.composer_mode == .history and key.key != .escape) {
            h.setComposerMode(.normal);
        }

        if (key.char != null and h.prompt.gap.length() >= HomeView.max_prompt_bytes) {
            h.setComposerMode(.@"error");
            h.setNotice("Prompt limit reached (512 bytes).");
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

        // Tab cycles agent mode: build -> plan -> code -> build
        if (key.key == .tab and !key.shift) {
            const current = s.footer.info.mode;
            s.footer.info.mode = if (std.mem.eql(u8, current, "build"))
                "plan"
            else if (std.mem.eql(u8, current, "plan"))
                "code"
            else
                "build";
            return;
        }

        // Shift+Tab cycles sidebar tabs: conversation -> changes -> diff
        if (key.key == .shift_tab) {
            s.cycleSidebarTab(1);
            return;
        }

        // Left/Right arrows cycle sidebar tabs when not typing in prompt
        if (key.key == .left and !key.ctrl and s.prompt.gap.length() == 0) {
            s.cycleSidebarTab(-1);
            return;
        }
        if (key.key == .right and !key.ctrl and s.prompt.gap.length() == 0) {
            s.cycleSidebarTab(1);
            return;
        }

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
        switch (self.ui.route) {
            .session => {
                if (self.session) |*s| {
                    if (mouse.btn == .scroll_up) {
                        s.scroll.scrollUp();
                    } else if (mouse.btn == .scroll_down) {
                        s.scroll.scrollDown();
                    } else if (mouse.btn == .left) {
                        // Check if click is on header row (y == 0)
                        if (mouse.row == 0) {
                            const sidebar_w: u16 = if (s.sidebar_visible) s.sidebar.width + 2 else 0;
                            const content_width = self.terminal.size.cols -| sidebar_w;
                            _ = s.handleHeaderClick(mouse.col, content_width);
                        }
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
