const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const buffer_mod = @import("../core/buffer.zig");
const Cell = cell_mod.Cell;
const Style = cell_mod.Style;
const Buffer = buffer_mod.Buffer;

pub const SpinnerStyle = enum {
    dots,
    line,
    pipe,
    arc,
    bounce,
    clock,
    earth,
    moon,
    weather,
    aesthetic,
};

const spinner_frames = [9][]const u8{
    "⠋",
    "⠙",
    "⠹",
    "⠸",
    "⠼",
    "⠴",
    "⠦",
    "⠧",
    "⠇",
};

const line_frames = [4][]const u8{ "-", "\\", "|", "/" };
const pipe_frames = [4][]const u8{ "┤", "┘", "└", "├" };
const arc_frames = [8][]const u8{ "◜", "◝", "◞", "◟", "◜", "◝", "◞", "◟" };
const bounce_frames = [8][]const u8{ "⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈" };
const clock_frames = [12][]const u8{ "🕛", "🕐", "二季度", "🕒", "🕓", "🕔", "🕕", "🕖", "🕗", "🕘", "🕙", "🕚" };
const earth_frames = [8][]const u8{ "🌍", "🌎", "🌏", "🌍", "🌎", "🌏", "🌍", "🌎" };
const moon_frames = [8][]const u8{ "🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘" };
const weather_frames = [4][]const u8{ "☀️", "🌤️", "⛅", "🌥️" };
const aesthetic_frames = [8][]const u8{ " Blanco", " BlanEc", " Blanca", " BlanCi", " BlancIn", " BlancIo", " Blanciu", " Blanco" };

pub const Spinner = struct {
    style: SpinnerStyle,
    frame: u32,
    label: []const u8,
    label_style: Style,
    spinner_style: Style,
    speed_ms: u32,

    pub fn init(label: []const u8, spinner_style: Style, label_style: Style) Spinner {
        return .{
            .style = .dots,
            .frame = 0,
            .label = label,
            .label_style = label_style,
            .spinner_style = spinner_style,
            .speed_ms = 80,
        };
    }

    pub fn withStyle(self: Spinner, s: SpinnerStyle) Spinner {
        var sp = self;
        sp.style = s;
        return sp;
    }

    pub fn withSpeed(self: Spinner, ms: u32) Spinner {
        var sp = self;
        sp.speed_ms = ms;
        return sp;
    }

    pub fn tick(self: *Spinner) void {
        self.frame +%= 1;
    }

    pub fn currentFrame(self: Spinner) []const u8 {
        return switch (self.style) {
            .dots => spinner_frames[self.frame % spinner_frames.len],
            .line => line_frames[self.frame % line_frames.len],
            .pipe => pipe_frames[self.frame % pipe_frames.len],
            .arc => arc_frames[self.frame % arc_frames.len],
            .bounce => bounce_frames[self.frame % bounce_frames.len],
            .clock => clock_frames[self.frame % clock_frames.len],
            .earth => earth_frames[self.frame % earth_frames.len],
            .moon => moon_frames[self.frame % moon_frames.len],
            .weather => weather_frames[self.frame % weather_frames.len],
            .aesthetic => aesthetic_frames[self.frame % aesthetic_frames.len],
        };
    }

    pub fn render(self: Spinner, buf: *Buffer, x: u16, y: u16, max_width: u16) void {
        const frame = self.currentFrame();
        const total = frame.len + 1 + self.label.len;
        const display_w: u16 = @intCast(@min(total, max_width));

        // Spinner animation
        _ = buf.writeStringBounded(x, y, frame, self.spinner_style, display_w);

        // Label
        const label_start = @min(x + @as(u16, @intCast(frame.len)), x + display_w);
        const label_w: u16 = display_w -| @as(u16, @intCast(frame.len));
        if (label_w > 0) {
            _ = buf.writeStringBounded(label_start, y, self.label, self.label_style, label_w);
        }
    }

    /// Render centered
    pub fn renderCentered(self: Spinner, buf: *Buffer, y: u16, available_width: u16) void {
        const frame = self.currentFrame();
        const total_w = @as(u16, @intCast(frame.len)) + 1 + @as(u16, @intCast(self.label.len));
        const start_x = if (total_w < available_width) (available_width - total_w) / 2 else 0;
        self.render(buf, start_x, y, available_width);
    }
};
