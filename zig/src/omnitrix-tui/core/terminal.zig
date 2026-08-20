const std = @import("std");
const linux = std.os.linux;
const buffer_mod = @import("buffer.zig");
const Buffer = buffer_mod.Buffer;
const cell_mod = @import("cell.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const theme_mod = @import("theme.zig");
const Theme = theme_mod.Theme;
const ThemeMode = theme_mod.ThemeMode;

pub const TerminalSize = struct {
    cols: u16,
    rows: u16,
};

pub const MouseBtn = enum {
    left,
    middle,
    right,
    release,
    scroll_up,
    scroll_down,
};

pub const MouseEvent = struct {
    col: u16,
    row: u16,
    btn: MouseBtn,
    ctrl: bool,
    shift: bool,
    alt: bool,
};

pub const KeyEvent = struct {
    char: ?u21 = null,
    key: SpecialKey = .none,
    ctrl: bool = false,
    alt: bool = false,
    shift: bool = false,

    pub const SpecialKey = enum {
        none,
        enter,
        escape,
        backspace,
        tab,
        shift_tab,
        up,
        down,
        left,
        right,
        home,
        end,
        page_up,
        page_down,
        delete,
        insert,
        f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12,
        mouse,
    };
};

pub const InputEvent = union(enum) {
    key: KeyEvent,
    mouse: MouseEvent,
    resize: TerminalSize,
};

/// Termios captured before the TUI switched the terminal into raw mode.
/// Kept at module scope so a panic or fatal signal can restore the terminal
/// even when the `Terminal` instance itself is gone or corrupted.
var saved_termios: ?std.os.linux.termios = null;

/// Sequence that undoes every mode the TUI turns on, in reverse order:
/// leave alt screen, disable all mouse reporting, show cursor, reset SGR.
const restore_sequence = "\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?25h\x1b[0m";

/// Put the terminal back into a usable state. Safe to call from a panic or
/// signal handler: no allocation, no locks, only raw syscalls.
///
/// Without this, a crash leaves mouse tracking enabled and the terminal keeps
/// emitting SGR mouse reports (`ESC[<0;50;38M`) which the shell then prints as
/// garbage like `0;50;38M32;49;38M...`.
pub fn emergencyRestore() void {
    _ = linux.write(1, restore_sequence.ptr, restore_sequence.len);
    if (saved_termios) |*t| {
        _ = linux.tcsetattr(0, .NOW, t);
    }
}

fn fatalSignalHandler(sig: std.posix.SIG) callconv(.c) void {
    emergencyRestore();
    linux.exit(128 + @as(i32, @intCast(@intFromEnum(sig))));
}

/// Restore the terminal even when the process is killed from outside
/// (closing the tab, `timeout`, `kill`). Without this the shell is left in
/// raw mode with mouse tracking on and spews SGR reports as garbage.
pub fn installFatalSignalHandlers() void {
    const act = std.posix.Sigaction{
        .handler = .{ .handler = fatalSignalHandler },
        .mask = std.mem.zeroes(std.os.linux.sigset_t),
        .flags = 0,
    };
    std.posix.sigaction(.TERM, &act, null);
    std.posix.sigaction(.HUP, &act, null);
    std.posix.sigaction(.QUIT, &act, null);
}

/// Bytes read from stdin but not yet consumed as a complete input event.
/// Keeps fast/multi-byte input (pastes, several keys at once) intact instead
/// of dropping everything after the first event of a single read().
const PendingInput = struct {
    data: [2048]u8 = undefined,
    len: usize = 0,

    fn append(self: *PendingInput, bytes: []const u8) void {
        if (self.len + bytes.len > self.data.len) {
            // Runaway input: drop everything and start fresh rather than
            // letting stale garbage wedge the parser.
            self.len = 0;
        }
        @memcpy(self.data[self.len..][0..bytes.len], bytes);
        self.len += bytes.len;
    }
};

pub const Terminal = struct {
    allocator: std.mem.Allocator,
    original_termios: std.os.linux.termios,
    buffer: Buffer,
    size: TerminalSize,
    stdin_fd: i32,
    stdout_fd: i32,
    raw_mode: bool,
    alternate_screen: bool,
    mouse_tracking: bool,
    /// True when the terminal understands the Kitty graphics protocol;
    /// detected once at startup from the environment.
    kitty_graphics: bool,
    pending_input: PendingInput = .{},

    pub fn init(allocator: std.mem.Allocator) !Terminal {
        const stdin: i32 = 0;
        const stdout: i32 = 1;

        // A TUI is meaningless without a controlling terminal. Fail fast with a
        // clear error instead of rendering escape codes into a pipe/file.
        var original: std.os.linux.termios = std.mem.zeroes(std.os.linux.termios);
        if (linux.errno(linux.tcgetattr(stdin, &original)) != .SUCCESS) {
            return error.NotATerminal;
        }
        // Publish the pristine termios before touching any terminal mode, so a
        // crash at any point from here on can still restore the shell.
        saved_termios = original;
        installFatalSignalHandlers();

        const size = getSize(stdout) catch
            getSize(stdin) catch
            TerminalSize{ .cols = 80, .rows = 24 };

        var raw = original;
        raw.iflag.ICRNL = false;
        raw.iflag.IXON = false;
        raw.iflag.IXOFF = false;
        raw.iflag.ISTRIP = false;
        raw.iflag.IUCLC = false;
        raw.iflag.IMAXBEL = false;
        raw.lflag.ECHO = false;
        raw.lflag.ICANON = false;
        raw.lflag.ISIG = false;
        raw.lflag.IEXTEN = false;
        raw.lflag.ECHONL = false;
        raw.lflag.ECHOCTL = false;
        raw.lflag.ECHOKE = false;
        raw.lflag.ECHOPRT = false;
        raw.lflag.ECHOE = false;
        raw.lflag.ECHOK = false;
        raw.cc[@intFromEnum(std.os.linux.V.MIN)] = 0;
        raw.cc[@intFromEnum(std.os.linux.V.TIME)] = 0;
        _ = linux.tcsetattr(stdin, .NOW, &raw);
        // If anything below fails we must not leave the user's shell in raw mode.
        errdefer _ = linux.tcsetattr(stdin, .NOW, &original);

        const buf = try Buffer.init(allocator, size.cols, size.rows);

        return .{
            .allocator = allocator,
            .original_termios = original,
            .buffer = buf,
            .size = size,
            .stdin_fd = stdin,
            .stdout_fd = stdout,
            .raw_mode = true,
            .alternate_screen = false,
            .mouse_tracking = false,
            .kitty_graphics = detectKittyGraphics(),
        };
    }

    pub fn deinit(self: *Terminal) void {
        self.leaveAlternateScreen();
        self.disableMouseTracking();
        self.showCursor();
        self.buffer.deinit();
        _ = linux.tcsetattr(self.stdin_fd, .NOW, &self.original_termios);
    }

    pub fn getSize(fd: i32) !TerminalSize {
        var ws: std.posix.winsize = undefined;
        const rc = linux.ioctl(fd, std.os.linux.T.IOCGWINSZ, @intFromPtr(&ws));
        if (rc != 0 or ws.col == 0 or ws.row == 0) return error.InvalidSize;
        return .{ .cols = ws.col, .rows = ws.row };
    }

    /// Refresh the cached terminal size. A failing ioctl is *not* fatal: the
    /// previously known size is kept so a transient failure cannot tear down
    /// the whole render loop.
    pub fn updateSize(self: *Terminal) !TerminalSize {
        const new_size = getSize(self.stdout_fd) catch
            getSize(self.stdin_fd) catch
            return self.size;
        if (new_size.cols != self.size.cols or new_size.rows != self.size.rows) {
            self.size = new_size;
            try self.buffer.resize(new_size.cols, new_size.rows);
        }
        return new_size;
    }

    pub fn enterAlternateScreen(self: *Terminal) void {
        if (self.alternate_screen) return;
        self.writeStr("\x1b[?1049h");
        self.alternate_screen = true;
    }

    pub fn leaveAlternateScreen(self: *Terminal) void {
        if (!self.alternate_screen) return;
        self.writeStr("\x1b[?1049l");
        self.alternate_screen = false;
    }

    pub fn enableMouseTracking(self: *Terminal) void {
        if (self.mouse_tracking) return;
        self.writeStr("\x1b[?1000h\x1b[?1002h\x1b[?1006h");
        self.mouse_tracking = true;
    }

    pub fn disableMouseTracking(self: *Terminal) void {
        if (!self.mouse_tracking) return;
        self.writeStr("\x1b[?1000l\x1b[?1002l\x1b[?1006l");
        self.mouse_tracking = false;
    }

    pub fn showCursor(self: *Terminal) void {
        self.writeStr("\x1b[?25h");
    }

    pub fn hideCursor(self: *Terminal) void {
        self.writeStr("\x1b[?25l");
    }

    pub fn moveCursorTo(self: *Terminal, col: u16, row: u16) void {
        var buf: [32]u8 = undefined;
        const s = std.fmt.bufPrint(&buf, "\x1b[{d};{d}H", .{ row + 1, col + 1 }) catch return;
        self.writeStr(s);
    }

    pub fn clearScreen(self: *Terminal) void {
        self.writeStr("\x1b[2J\x1b[H");
    }

    pub fn writeStr(self: *Terminal, s: []const u8) void {
        _ = linux.write(self.stdout_fd, s.ptr, s.len);
    }

    /// Render Kitty graphics protocol command at the specified cell position.
    /// Kitty is supported by Ghostty, Kitty, WezTerm and other modern terminals.
    pub fn renderKittyAt(self: *Terminal, col: u16, row: u16, kitty_cmd: []const u8) void {
        // Move cursor to position
        self.moveCursorTo(col, row);
        // Send Kitty graphics command
        self.writeStr(kitty_cmd);
    }

    /// Detect Kitty graphics support from the environment, once at startup.
    ///
    /// The previous implementation queried the terminal interactively
    /// (ESC_G a=q) and did a raw read() for the answer. In raw mode with
    /// VMIN=0/VTIME=0 that read raced the terminal's response (almost
    /// always returning 0 bytes) and, when it did return, swallowed real
    /// input bytes. Env-based detection has neither problem.
    pub fn detectKittyGraphics() bool {
        var env_buf: [32 * 1024]u8 = undefined;
        const environ = env_buf[0..readProcEnviron(&env_buf)];

        // Explicit kill switch (the screenshot rig sets it: pyte renders
        // Kitty payloads as garbage text).
        if (findEnvValue(environ, "OMNITRIX_NO_KITTY") != null) return false;
        // kitty sets KITTY_WINDOW_ID; Ghostty/WezTerm set TERM_PROGRAM.
        if (findEnvValue(environ, "KITTY_WINDOW_ID") != null) return true;
        if (findEnvValue(environ, "TERM_PROGRAM")) |prog| {
            if (std.mem.eql(u8, prog, "kitty")) return true;
            if (std.mem.eql(u8, prog, "ghostty")) return true;
            if (std.mem.eql(u8, prog, "WezTerm")) return true;
        }
        if (findEnvValue(environ, "TERM")) |term| {
            if (std.mem.indexOf(u8, term, "kitty") != null) return true;
            if (std.mem.indexOf(u8, term, "ghostty") != null) return true;
        }
        return false;
    }

    /// Read the process environment from /proc/self/environ into buf and
    /// return how many bytes were read. Done by hand because this Zig dev
    /// build no longer exposes std.process.getenv and the binary is built
    /// without libc (so std.c.getenv is unavailable too). The rest of this
    /// file already talks to the kernel via raw linux syscalls.
    fn readProcEnviron(buf: []u8) usize {
        // In this Zig dev build `linux.O` is a packed struct whose defaults
        // are read-only, and the path must be NUL-terminated.
        const rc = linux.open("/proc/self/environ", .{}, 0);
        if (linux.errno(rc) != .SUCCESS) return 0;
        const fd: i32 = @intCast(rc);
        defer _ = linux.close(fd);
        var total: usize = 0;
        while (total < buf.len) {
            const n = linux.read(fd, buf[total..].ptr, buf.len - total);
            if (linux.errno(n) != .SUCCESS or n == 0) break;
            total += n;
        }
        return total;
    }

    /// Look up NAME=VALUE in a NUL-separated environ blob; returns VALUE.
    fn findEnvValue(environ: []const u8, name: []const u8) ?[]const u8 {
        var start: usize = 0;
        while (start < environ.len) {
            const end = std.mem.indexOfScalarPos(u8, environ, start, 0) orelse environ.len;
            const entry = environ[start..end];
            if (entry.len > name.len and std.mem.startsWith(u8, entry, name) and entry[name.len] == '=') {
                return entry[name.len + 1 ..];
            }
            start = end + 1;
        }
        return null;
    }

    pub fn flush(self: *Terminal) !void {
        try self.buffer.flush(Writer{ .fd = self.stdout_fd });
    }

    pub fn readEvent(self: *Terminal) ?InputEvent {
        // Prefer parsing from leftover bytes before issuing a new read().
        // Handles the common case where a single read() carried several
        // events (paste, fast typing): each is parsed one at a time.
        while (true) {
            if (self.pending_input.len > 0) {
                const res = self.parseInput(self.pending_input.data[0..self.pending_input.len]);
                const consumed = res.consumed;
                if (consumed > 0) {
                    const keep = self.pending_input.len - consumed;
                    if (keep > 0) {
                        std.mem.copyForwards(u8, self.pending_input.data[0..keep], self.pending_input.data[consumed..self.pending_input.len]);
                    }
                    self.pending_input.len = keep;
                    if (res.event != null) return res.event;
                    continue; // consumed junk, try the next event
                }
                if (res.event != null) return res.event;
                // consumed == 0 and no event: an incomplete escape sequence.
                // Read more bytes to try to complete it.
            }

            var buf: [128]u8 = undefined;
            // VMIN=0/TIME=0 make this read non-blocking, so a missing
            // sequence end can never hang the render loop.
            const rc = linux.read(self.stdin_fd, &buf, buf.len);
            if (linux.errno(rc) != .SUCCESS) return null;
            if (rc == 0) return null;
            self.pending_input.append(buf[0..rc]);
        }
    }

    /// Wait up to `timeout_ms` for input, then read it.
    ///
    /// Uses a real poll(2) so the loop sleeps in the kernel until either input
    /// arrives or the timeout expires. `readEvent` is only called once poll
    /// reports readiness, so it can never block the render loop.
    pub fn readEventTimeout(self: *Terminal, timeout_ms: u32) ?InputEvent {
        // Drain leftover bytes from a previous burst read BEFORE polling.
        // poll(2) only wakes on *new* fd data, so if a single read() carried
        // several events (pasted text, fast typing) and only the first was
        // returned, the rest would starve in pending_input until unrelated
        // input happened to arrive -- the "only the first character appears"
        // bug in the command palette.
        if (self.pending_input.len > 0) {
            if (self.readEvent()) |ev| return ev;
        }

        var pollfds = [_]linux.pollfd{.{
            .fd = self.stdin_fd,
            .events = linux.POLL.IN,
            .revents = 0,
        }};

        const timeout: i32 = std.math.cast(i32, timeout_ms) orelse std.math.maxInt(i32);

        while (true) {
            const rc = linux.poll(&pollfds, pollfds.len, timeout);
            switch (linux.errno(rc)) {
                .SUCCESS => {},
                // Interrupted by a signal (e.g. SIGWINCH): let the caller
                // re-render, it will poll again on the next frame.
                .INTR => return null,
                else => return null,
            }
            if (rc == 0) return null; // timed out, no input pending
            const ready = pollfds[0].revents & (linux.POLL.IN | linux.POLL.HUP | linux.POLL.ERR);
            if (ready == 0) return null;
            return self.readEvent();
        }
    }

    const ParseResult = struct {
        event: ?InputEvent,
        consumed: usize,
    };

    fn parseInput(self: *Terminal, data: []const u8) ParseResult {
        _ = self;
        if (data.len == 0) return .{ .event = null, .consumed = 0 };

        // SGR mouse: ESC [ < b;c;r M|m
        if (data.len >= 3 and data[0] == '\x1b' and data[1] == '[' and data[2] == '<') {
            var i: usize = 3;
            while (i < data.len and data[i] != 'M' and data[i] != 'm') : (i += 1) {}
            if (i >= data.len) return .{ .event = null, .consumed = 0 };
            return .{ .event = parseSgrMouse(data[0 .. i + 1]), .consumed = i + 1 };
        }

        // CSI: ESC [ params final-byte (0x40..0x7E)
        if (data.len >= 2 and data[0] == '\x1b' and data[1] == '[') {
            var i: usize = 2;
            while (i < data.len and !(data[i] >= 0x40 and data[i] <= 0x7e)) : (i += 1) {}
            if (i >= data.len) return .{ .event = null, .consumed = 0 };
            return .{ .event = parseCSI(data[0 .. i + 1]), .consumed = i + 1 };
        }

        // ESC followed by a printable character: Alt+key.
        if (data[0] == '\x1b' and data.len >= 2) {
            if (data[1] >= 0x20) {
                return .{ .event = .{ .key = .{ .char = data[1], .alt = true } }, .consumed = 2 };
            }
            // ESC + control byte: nothing meaningful, drop the ESC alone.
            return .{ .event = .{ .key = .{ .key = .escape } }, .consumed = 1 };
        }

        // Lone ESC = the Escape key.
        if (data.len == 1 and data[0] == '\x1b') {
            return .{ .event = .{ .key = .{ .key = .escape } }, .consumed = 1 };
        }

        if (data[0] >= 1 and data[0] <= 26) {
            const ch = data[0];
            if (ch == 13 or ch == 10) return .{ .event = .{ .key = .{ .key = .enter } }, .consumed = 1 };
            if (ch == 9) return .{ .event = .{ .key = .{ .key = .tab } }, .consumed = 1 };
            if (ch == 127) return .{ .event = .{ .key = .{ .key = .backspace } }, .consumed = 1 };
            return .{ .event = .{ .key = .{ .char = 'a' + ch - 1, .ctrl = true } }, .consumed = 1 };
        }

        if (data[0] == 13 or data[0] == 10) return .{ .event = .{ .key = .{ .key = .enter } }, .consumed = 1 };
        if (data[0] == 9) return .{ .event = .{ .key = .{ .key = .tab } }, .consumed = 1 };
        if (data[0] == 127) return .{ .event = .{ .key = .{ .key = .backspace } }, .consumed = 1 };
        if (data[0] == 27) return .{ .event = .{ .key = .{ .key = .escape } }, .consumed = 1 };

        if (data[0] >= 0x80) {
            const n = std.unicode.utf8ByteSequenceLength(data[0]) catch 1;
            if (data.len < n) return .{ .event = null, .consumed = 0 }; // truncated UTF-8: wait for more
            const cp = std.unicode.wtf8Decode(data[0..n]) catch return .{ .event = null, .consumed = 1 };
            return .{ .event = .{ .key = .{ .char = cp } }, .consumed = n };
        }

        if (data[0] >= 0x20) {
            return .{ .event = .{ .key = .{ .char = data[0] } }, .consumed = 1 };
        }

        return .{ .event = null, .consumed = 1 };
    }

    fn parseSgrMouse(data: []const u8) ?InputEvent {
        var i: usize = 3;
        var btn_val: u32 = 0;
        while (i < data.len and data[i] != ';') : (i += 1) {
            if (data[i] >= '0' and data[i] <= '9') btn_val = btn_val * 10 + (data[i] - '0');
        }
        if (i >= data.len) return null;
        i += 1;
        var col_val: u32 = 0;
        while (i < data.len and data[i] != ';') : (i += 1) {
            if (data[i] >= '0' and data[i] <= '9') col_val = col_val * 10 + (data[i] - '0');
        }
        if (i >= data.len) return null;
        i += 1;
        var row_val: u32 = 0;
        while (i < data.len and data[i] != 'M' and data[i] != 'm') : (i += 1) {
            if (data[i] >= '0' and data[i] <= '9') row_val = row_val * 10 + (data[i] - '0');
        }
        if (i >= data.len) return null;

        const btn: MouseBtn = switch (btn_val & 3) {
            0 => if ((btn_val >> 6) & 1 == 1) .scroll_up else .left,
            1 => if ((btn_val >> 6) & 1 == 1) .scroll_down else .middle,
            2 => .right,
            else => .release,
        };

        return .{ .mouse = .{
            .col = @intCast(@min(col_val, 65535)),
            .row = @intCast(@min(row_val, 65535)),
            .btn = btn,
            .ctrl = (btn_val & 4) != 0,
            .shift = (btn_val & 8) != 0,
            .alt = (btn_val & 16) != 0,
        } };
    }

    fn parseCSI(data: []const u8) ?InputEvent {
        if (data.len < 3) return null;
        const last = data[data.len - 1];

        if (data[2] == '1' and data.len >= 5 and data[3] == ';') {
            var mod: u32 = 0;
            var i: usize = 4;
            while (i < data.len - 1) : (i += 1) {
                if (data[i] >= '0' and data[i] <= '9') mod = mod * 10 + (data[i] - '0');
            }
            const shift = (mod & 1) != 0;
            const alt = (mod & 2) != 0;
            const ctrl = (mod & 4) != 0;
            const key = switch (last) {
                'A' => KeyEvent.SpecialKey.up,
                'B' => KeyEvent.SpecialKey.down,
                'C' => KeyEvent.SpecialKey.right,
                'D' => KeyEvent.SpecialKey.left,
                'H' => KeyEvent.SpecialKey.home,
                'F' => KeyEvent.SpecialKey.end,
                else => return parseCSIBasic(data, last),
            };
            return .{ .key = .{ .key = key, .ctrl = ctrl, .alt = alt, .shift = shift } };
        }

        if (data.len == 3) {
            return switch (last) {
                'A' => .{ .key = .{ .key = .up } },
                'B' => .{ .key = .{ .key = .down } },
                'C' => .{ .key = .{ .key = .right } },
                'D' => .{ .key = .{ .key = .left } },
                'H' => .{ .key = .{ .key = .home } },
                'F' => .{ .key = .{ .key = .end } },
                'Z' => .{ .key = .{ .key = .shift_tab } },
                else => null,
            };
        }

        return parseCSIBasic(data, last);
    }

    fn parseCSIBasic(data: []const u8, last: u8) ?InputEvent {
        if (last != '~') return null;
        var num: u32 = 0;
        for (2..data.len - 1) |idx| {
            if (data[idx] >= '0' and data[idx] <= '9') {
                num = num * 10 + (data[idx] - '0');
            }
        }
        return switch (num) {
            1 => .{ .key = .{ .key = .home } },
            2 => .{ .key = .{ .key = .insert } },
            3 => .{ .key = .{ .key = .delete } },
            4 => .{ .key = .{ .key = .end } },
            5 => .{ .key = .{ .key = .page_up } },
            6 => .{ .key = .{ .key = .page_down } },
            else => blk: {
                if (num >= 11 and num <= 24) {
                    break :blk .{ .key = .{ .key = @enumFromInt(@as(u8, @intCast(num - 11 + @as(u32, @intFromEnum(KeyEvent.SpecialKey.f1))))) } };
                }
                break :blk null;
            },
        };
    }
};

pub const Writer = struct {
    fd: i32,

    pub fn writeAll(self: Writer, bytes: []const u8) !void {
        var written: usize = 0;
        while (written < bytes.len) {
            const n = linux.write(self.fd, bytes.ptr + written, bytes.len - written);
            if (n <= 0) return error.WriteFailed;
            written += @intCast(n);
        }
    }

    pub fn writeByte(self: Writer, byte: u8) !void {
        try self.writeAll(&.{byte});
    }

    pub fn writeAllNTimes(self: Writer, bytes: []const u8, n: usize) !void {
        for (0..n) |_| {
            try self.writeAll(bytes);
        }
    }
};
