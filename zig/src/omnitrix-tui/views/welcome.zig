const std = @import("std");
const buffer_mod = @import("../core/buffer.zig");
const cell_mod = @import("../core/cell.zig");
const logo_mod = @import("../core/logo.zig");
const term_mod = @import("../core/terminal.zig");
const theme_mod = @import("../core/theme.zig");
const footer_mod = @import("footer.zig");

const Buffer = buffer_mod.Buffer;
const KeyEvent = term_mod.KeyEvent;
const Style = cell_mod.Style;
const Theme = theme_mod.Theme;

pub const AuthState = enum {
    signed_out,
    choosing,
    pending,
    authenticated,
    failed,
};

pub const AuthMethod = enum { browser, device_code, api_key };

pub const WelcomeAction = union(enum) {
    none,
    choose_method: AuthMethod,
    begin_auth: AuthMethod,
    cancel,
    complete,
    retry,
};

pub const WelcomeView = struct {
    auth_state: AuthState = .signed_out,
    selected_method: u8 = 0,
    notice_len: u8 = 0,
    notice: [160]u8 = undefined,

    pub fn init() WelcomeView {
        var view = WelcomeView{};
        view.setState(.signed_out, "Choose a fixture sign-in method to continue.");
        return view;
    }

    pub fn deinit(self: *WelcomeView) void {
        _ = self;
    }

    pub fn setState(self: *WelcomeView, state: AuthState, message: []const u8) void {
        self.auth_state = state;
        const length = @min(message.len, self.notice.len);
        @memcpy(self.notice[0..length], message[0..length]);
        self.notice_len = @intCast(length);
    }

    pub fn handleKey(self: *WelcomeView, key: KeyEvent) WelcomeAction {
        switch (self.auth_state) {
            .signed_out => {
                if (key.key == .enter or key.char == 'l' or key.char == 'L') {
                    return .{ .choose_method = .browser };
                }
                if (methodFromKey(key.char)) |method| {
                    self.selected_method = @intFromEnum(method);
                    return .{ .choose_method = method };
                }
            },
            .choosing => {
                if (key.key == .escape) return .cancel;
                if (methodFromKey(key.char)) |method| {
                    self.selected_method = @intFromEnum(method);
                    return .{ .choose_method = method };
                }
                if (key.key == .up or key.key == .left) {
                    self.selected_method = if (self.selected_method == 0) 2 else self.selected_method - 1;
                    return .{ .choose_method = selectedMethod(self.selected_method) };
                }
                if (key.key == .down or key.key == .right) {
                    self.selected_method = (self.selected_method + 1) % 3;
                    return .{ .choose_method = selectedMethod(self.selected_method) };
                }
                if (key.key == .enter) return .{ .begin_auth = selectedMethod(self.selected_method) };
            },
            .pending => {
                if (key.key == .escape) return .cancel;
                if (key.key == .enter) return .complete;
                // This deterministic failure key keeps the error path visible
                // without contacting a provider or accepting credentials.
                if (key.char == 'f' or key.char == 'F') {
                    self.setState(.failed, "Fixture handoff failed. Press R to retry or Esc to cancel.");
                }
            },
            .failed => {
                if (key.key == .escape) return .cancel;
                if (key.char == 'r' or key.char == 'R' or key.key == .enter) return .retry;
            },
            .authenticated => {},
        }
        return .none;
    }

    pub fn render(self: *const WelcomeView, buf: *Buffer, width: u16, height: u16, theme: Theme) void {
        buf.fillRegion(0, 0, width, height, .{ .style = .{ .bg = theme.background } });
        if (width == 0 or height == 0) return;

        const logo_style = Style{ .fg = theme.accent, .bg = theme.background };
        const logo_width = buffer_mod.stringWidth(logo_mod.OMNITRIX[0]);
        const logo_x = if (logo_width < width) (width - logo_width) / 2 else 0;
        const block_height: u16 = 6 + 2 + 9;
        const start_y = if (block_height < height) (height - block_height) / 2 else 1;

        for (logo_mod.OMNITRIX, 0..) |line, index| {
            const y = start_y + @as(u16, @intCast(index));
            if (y >= height) break;
            _ = buf.writeStringBounded(logo_x, y, line, logo_style, width -| logo_x);
        }

        const brand_y = start_y + 7;
        writeCentered(buf, brand_y, width, "OMNITRIX", Style{ .fg = theme.text, .bg = theme.background, .attr = .{ .bold = true } });
        const state_y = brand_y + 2;
        writeCentered(buf, state_y, width, stateLabel(self.auth_state), Style{ .fg = theme.text_muted, .bg = theme.background });

        const method_y = state_y + 2;
        if (self.auth_state == .pending) {
            writeCentered(buf, method_y, width, pendingLabel(self.selected_method), Style{ .fg = theme.info, .bg = theme.background });
        } else {
            const methods = [_][]const u8{
                "1  Browser sign-in",
                "2  Device code handoff",
                "3  API key  ********  (visual only)",
            };
            for (methods, 0..) |label, index| {
                const y = method_y + @as(u16, @intCast(index));
                if (y >= height -| 2) break;
                const selected = self.auth_state == .choosing and self.selected_method == index;
                const style = if (selected)
                    Style{ .fg = theme.background, .bg = theme.accent, .attr = .{ .bold = true } }
                else
                    Style{ .fg = theme.text_muted, .bg = theme.background };
                writeCentered(buf, y, width, label, style);
            }
        }

        if (self.notice_len > 0) {
            const notice_y = height -| 3;
            writeCentered(buf, notice_y, width, self.notice[0..self.notice_len], .{ .fg = noticeColor(self.auth_state, theme), .bg = theme.background });
        }
        footer_mod.renderWelcomeContext(buf, height -| 1, width, theme, @intFromEnum(self.auth_state));
    }

    fn methodFromKey(char: ?u21) ?AuthMethod {
        return switch (char orelse 0) {
            '1' => .browser,
            '2' => .device_code,
            '3' => .api_key,
            else => null,
        };
    }

    fn selectedMethod(index: u8) AuthMethod {
        return switch (index % 3) {
            0 => .browser,
            1 => .device_code,
            else => .api_key,
        };
    }

    fn stateLabel(state: AuthState) []const u8 {
        return switch (state) {
            .signed_out => "Sign in to start a fixture workspace",
            .choosing => "Choose how the fixture should continue",
            .pending => "Waiting for the fixture handoff",
            .authenticated => "Fixture sign-in complete",
            .failed => "Fixture sign-in unavailable",
        };
    }

    fn pendingLabel(index: u8) []const u8 {
        return switch (selectedMethod(index)) {
            .browser => "Browser handoff ready — press Enter to complete",
            .device_code => "Device-code handoff ready — press Enter to complete",
            .api_key => "Masked API-key fixture",
        };
    }

    fn noticeColor(state: AuthState, theme: Theme) @TypeOf(theme.accent) {
        return switch (state) {
            .failed => theme.err_color,
            .pending => theme.info,
            .authenticated => theme.success,
            else => theme.text_muted,
        };
    }

    fn writeCentered(buf: *Buffer, y: u16, width: u16, text: []const u8, style: Style) void {
        if (y >= buf.rows or width == 0) return;
        const text_width = buffer_mod.stringWidth(text);
        const x = if (text_width < width) (width - text_width) / 2 else 0;
        _ = buf.writeStringBounded(x, y, text, style, width -| x);
    }
};
