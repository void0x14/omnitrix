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

    pub fn init(allocator: std.mem.Allocator) !Terminal {
        const stdin: i32 = 0;
        const stdout: i32 = 1;

        const size = getSize(stdout) catch TerminalSize{ .cols = 80, .rows = 24 };

        var original: std.os.linux.termios = undefined;
        _ = linux.tcgetattr(stdin, &original);

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

    pub fn updateSize(self: *Terminal) !TerminalSize {
        const new_size = try getSize(self.stdin_fd);
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

    pub fn flush(self: *Terminal) !void {
        try self.buffer.flush(Writer{ .fd = self.stdout_fd });
    }

    pub fn readEvent(self: *Terminal) ?InputEvent {
        var buf: [64]u8 = undefined;
        const n = linux.read(self.stdin_fd, &buf, buf.len);
        if (n <= 0) return null;
        return self.parseInput(buf[0..@intCast(n)]);
    }

    pub fn readEventTimeout(self: *Terminal, timeout_ms: u32) ?InputEvent {
        var pollfds = [_]std.os.linux.pollfd{.{
            .fd = self.stdin_fd,
            .events = std.os.linux.POLL.IN,
            .revents = 0,
        }};
        const rc = linux.poll(&pollfds, 1, @intCast(timeout_ms));
        if (rc <= 0) return null;
        return self.readEvent();
    }

    fn parseInput(self: *Terminal, data: []const u8) ?InputEvent {
        _ = self;
        if (data.len == 0) return null;

        if (data.len >= 6 and data[0] == '\x1b' and data[1] == '[' and data[2] == '<') {
            return parseSgrMouse(data);
        }

        if (data[0] == '\x1b' and data.len >= 2 and data[1] == '[') {
            return parseCSI(data);
        }

        if (data[0] == '\x1b' and data.len >= 2 and data[1] != '[') {
            if (data.len == 2 and data[1] >= 0x20) {
                return .{ .key = .{ .char = data[1], .alt = true } };
            }
            return null;
        }

        if (data[0] >= 1 and data[0] <= 26) {
            const ch = data[0];
            if (ch == 13 or ch == 10) return .{ .key = .{ .key = .enter } };
            if (ch == 9) return .{ .key = .{ .key = .tab } };
            if (ch == 127) return .{ .key = .{ .key = .backspace } };
            return .{ .key = .{ .char = 'a' + ch - 1, .ctrl = true } };
        }

        if (data[0] == 13 or data[0] == 10) return .{ .key = .{ .key = .enter } };
        if (data[0] == 9) return .{ .key = .{ .key = .tab } };
        if (data[0] == 127) return .{ .key = .{ .key = .backspace } };
        if (data[0] == 27) return .{ .key = .{ .key = .escape } };

        if (data[0] >= 0x80) {
            const cp = std.unicode.wtf8Decode(data) catch return null;
            return .{ .key = .{ .char = cp } };
        }

        if (data[0] >= 0x20) {
            return .{ .key = .{ .char = data[0] } };
        }

        return null;
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
