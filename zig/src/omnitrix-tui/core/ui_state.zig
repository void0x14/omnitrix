pub const Route = enum { welcome, home, session, too_small };
pub const FocusTarget = enum { page, prompt, conversation, sidebar, overlay };
pub const UiStatus = enum { ready, loading, streaming, blocked, @"error" };

pub const Selection = struct {
    tab: u8 = 0,
    row: ?u16 = null,
    hunk: ?u16 = null,
    scroll_anchor: u32 = 0,
};

pub const Viewport = struct {
    cols: u16,
    rows: u16,

    pub const min_cols: u16 = 80;
    pub const min_rows: u16 = 24;

    pub fn isTooSmall(self: Viewport) bool {
        return self.cols < min_cols or self.rows < min_rows;
    }
};

pub const UiNotice = struct {
    kind: UiStatus = .ready,
    len: u8 = 0,
    bytes: [160]u8 = undefined,

    pub fn set(self: *UiNotice, kind: UiStatus, value: []const u8) void {
        const length = @min(value.len, self.bytes.len);
        @memcpy(self.bytes[0..length], value[0..length]);
        self.kind = kind;
        self.len = @intCast(length);
    }

    pub fn text(self: *const UiNotice) []const u8 {
        return self.bytes[0..self.len];
    }
};

pub const UiState = struct {
    route: Route = .welcome,
    focus: FocusTarget = .prompt,
    status: UiStatus = .ready,
    viewport: Viewport,
    selection: Selection = .{},
    notice: UiNotice = .{},

    pub fn setViewport(self: *UiState, cols: u16, rows: u16) void {
        self.viewport = .{ .cols = cols, .rows = rows };
    }

    pub fn clearNotice(self: *UiState) void {
        self.notice = .{};
    }
};
