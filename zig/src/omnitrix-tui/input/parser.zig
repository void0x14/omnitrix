//! omnitrix-tui/input/parser.zig
//!
//! VT500/ANSI ve xterm uyumlu Girdi Durum Makinesi (Input Parser FSM).
//! Terminalden gelen kesikli (chunked) bayt akışını deterministik olarak
//! klavye, fare, pencere ve yapıştırma olaylarına dönüştürür.

const std = @import("std");
const keys_mod = @import("keys.zig");

pub const Modifiers = keys_mod.Modifiers;
pub const SpecialKey = keys_mod.SpecialKey;
pub const KeyCode = keys_mod.KeyCode;
pub const KeyEvent = keys_mod.KeyEvent;
pub const MouseButton = keys_mod.MouseButton;
pub const MouseAction = keys_mod.MouseAction;
pub const MouseEvent = keys_mod.MouseEvent;
pub const InputEvent = keys_mod.InputEvent;

/// FSM Durumları
pub const ParserState = enum {
    ground,
    escape,
    csi_entry,
    csi_param,
    csi_intermediate,
    sgr_mouse,
    ss3,
    utf8_continuation,
};

pub const InputParser = struct {
    allocator: std.mem.Allocator,
    state: ParserState = .ground,

    // CSI parametre ayrıştırma tamponu
    params: [16]u16 = @splat(0),
    param_count: usize = 0,
    current_param: u16 = 0,
    has_param: bool = false,

    // Ara karakterler (Intermediate characters: space, $, ", vb.)
    intermediates: [4]u8 = @splat(0),
    intermediate_count: usize = 0,

    // UTF-8 çok baytlı tampon
    utf8_buf: [4]u8 = @splat(0),
    utf8_len: u3 = 0,
    utf8_expected: u3 = 0,

    // Bracketed paste durumu
    in_paste: bool = false,
    paste_buffer: std.ArrayList(u8),

    pub fn init(allocator: std.mem.Allocator) InputParser {
        return .{
            .allocator = allocator,
            .state = .ground,
            .params = @splat(0),
            .param_count = 0,
            .current_param = 0,
            .has_param = false,
            .intermediates = @splat(0),
            .intermediate_count = 0,
            .utf8_buf = @splat(0),
            .utf8_len = 0,
            .utf8_expected = 0,
            .in_paste = false,
            .paste_buffer = std.ArrayList(u8).empty,
        };
    }

    pub fn deinit(self: *InputParser) void {
        self.paste_buffer.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn reset(self: *InputParser) void {
        self.state = .ground;
        self.param_count = 0;
        self.current_param = 0;
        self.has_param = false;
        self.intermediate_count = 0;
        self.utf8_len = 0;
        self.utf8_expected = 0;
    }

    /// Bir bayt akışını ayrıştırır ve bulunan tüm olayları listeye ekler.
    pub fn parse(self: *InputParser, bytes: []const u8, out_events: *std.ArrayList(InputEvent)) !void {
        for (bytes) |b| {
            if (try self.processByte(b)) |event| {
                try out_events.append(self.allocator, event);
            }
        }
    }

    /// Tek bir baytı durum makinesinden geçirir.
    pub fn processByte(self: *InputParser, byte: u8) !?InputEvent {
        // Bracketed paste modu aktifse, bitiş dizisine (\x1b[201~) kadar topla
        if (self.in_paste) {
            return self.processPasteByte(byte);
        }

        switch (self.state) {
            .ground => return self.processGround(byte),
            .escape => return self.processEscape(byte),
            .csi_entry => return self.processCsiEntry(byte),
            .csi_param => return self.processCsiParam(byte),
            .csi_intermediate => return self.processCsiIntermediate(byte),
            .sgr_mouse => return self.processSgrMouse(byte),
            .ss3 => return self.processSs3(byte),
            .utf8_continuation => return self.processUtf8(byte),
        }
    }

    fn processGround(self: *InputParser, byte: u8) ?InputEvent {
        // 1. ESC karakteri
        if (byte == 0x1B) {
            self.state = .escape;
            return null;
        }

        // 2. ASCII Kontrol Karakterleri (C0)
        switch (byte) {
            0x00 => return .{ .key = KeyEvent.char(' ', .{ .ctrl = true }) }, // Ctrl+Space / Ctrl+@
            0x01...0x07, 0x0B, 0x0C, 0x0E...0x1A => {
                const c: u21 = @as(u21, byte) + 0x60; // 0x01 (Ctrl+A) -> 'a'
                return .{ .key = KeyEvent.char(c, .{ .ctrl = true }) };
            },
            0x08, 0x7F => return .{ .key = KeyEvent.special(.backspace, .none) },
            0x09 => return .{ .key = KeyEvent.special(.tab, .none) },
            0x0A, 0x0D => return .{ .key = KeyEvent.special(.enter, .none) },
            0x1C...0x1F => {
                // Ctrl+4..7 / Ctrl+\.._
                return .{ .key = KeyEvent.char(byte + 0x40, .{ .ctrl = true }) };
            },
            else => {},
        }

        // 3. Standart ASCII Yazdırılabilir Karakterler
        if (byte >= 0x20 and byte <= 0x7E) {
            return .{ .key = KeyEvent.char(byte, .none) };
        }

        // 4. UTF-8 Çok Baytlı Başlangıç
        const seq_len = std.unicode.utf8ByteSequenceLength(byte) catch 1;
        if (seq_len > 1 and seq_len <= 4) {
            self.utf8_buf[0] = byte;
            self.utf8_len = 1;
            self.utf8_expected = @intCast(seq_len);
            self.state = .utf8_continuation;
            return null;
        }

        return .{ .key = KeyEvent.char(byte, .none) };
    }

    fn processEscape(self: *InputParser, byte: u8) ?InputEvent {
        switch (byte) {
            '[' => {
                self.state = .csi_entry;
                self.param_count = 0;
                self.current_param = 0;
                self.has_param = false;
                self.intermediate_count = 0;
                return null;
            },
            'O' => {
                self.state = .ss3;
                return null;
            },
            0x1B => {
                // Çift ESC: Tek ESC dön
                self.state = .ground;
                return .{ .key = KeyEvent.special(.escape, .none) };
            },
            0x08, 0x7F => {
                // Alt + Backspace
                self.state = .ground;
                return .{ .key = KeyEvent.special(.backspace, .{ .alt = true }) };
            },
            0x0A, 0x0D => {
                // Alt + Enter
                self.state = .ground;
                return .{ .key = KeyEvent.special(.enter, .{ .alt = true }) };
            },
            0x01...0x07, 0x09, 0x0B, 0x0C, 0x0E...0x1A => {
                // Alt + Ctrl + Harf
                self.state = .ground;
                const c: u21 = @as(u21, byte) + 0x60;
                return .{ .key = KeyEvent.char(c, .{ .alt = true, .ctrl = true }) };
            },
            else => {
                self.state = .ground;
                if (byte >= 0x20 and byte <= 0x7E) {
                    return .{ .key = KeyEvent.char(byte, .{ .alt = true }) };
                }
                return .{ .key = KeyEvent.special(.escape, .none) };
            },
        }
    }

    fn processCsiEntry(self: *InputParser, byte: u8) ?InputEvent {
        if (byte == '<') {
            // SGR 1006 Mouse Mode
            self.state = .sgr_mouse;
            self.param_count = 0;
            self.current_param = 0;
            self.has_param = false;
            return null;
        }

        if (byte >= '0' and byte <= '9') {
            self.state = .csi_param;
            self.current_param = byte - '0';
            self.has_param = true;
            return null;
        }

        if (byte == ';') {
            self.state = .csi_param;
            self.commitParam();
            return null;
        }

        return self.finalizeCsi(byte);
    }

    fn processCsiParam(self: *InputParser, byte: u8) ?InputEvent {
        if (byte >= '0' and byte <= '9') {
            self.current_param = self.current_param * 10 + (byte - '0');
            self.has_param = true;
            return null;
        }

        if (byte == ';') {
            self.commitParam();
            return null;
        }

        if (byte >= 0x20 and byte <= 0x2F) {
            // Intermediate byte
            self.commitParam();
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            self.state = .csi_intermediate;
            return null;
        }

        self.commitParam();
        return self.finalizeCsi(byte);
    }

    fn processCsiIntermediate(self: *InputParser, byte: u8) ?InputEvent {
        if (byte >= 0x20 and byte <= 0x2F) {
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            return null;
        }
        return self.finalizeCsi(byte);
    }

    fn commitParam(self: *InputParser) void {
        if (self.param_count < self.params.len) {
            self.params[self.param_count] = if (self.has_param) self.current_param else 0;
            self.param_count += 1;
        }
        self.current_param = 0;
        self.has_param = false;
    }

    fn finalizeCsi(self: *InputParser, final_byte: u8) ?InputEvent {
        defer self.reset();

        const p1 = if (self.param_count > 0) self.params[0] else 0;
        const p2 = if (self.param_count > 1) self.params[1] else 0;

        const mod = parseCsiModifier(p2);

        switch (final_byte) {
            // Standart Ok Tuşları
            'A' => return .{ .key = KeyEvent.special(.up, if (p1 == 1) mod else .none) },
            'B' => return .{ .key = KeyEvent.special(.down, if (p1 == 1) mod else .none) },
            'C' => return .{ .key = KeyEvent.special(.right, if (p1 == 1) mod else .none) },
            'D' => return .{ .key = KeyEvent.special(.left, if (p1 == 1) mod else .none) },

            'H' => return .{ .key = KeyEvent.special(.home, mod) },
            'F' => return .{ .key = KeyEvent.special(.end, mod) },
            'Z' => return .{ .key = KeyEvent.special(.backtab, .{ .shift = true }) },

            // Focus In / Focus Out
            'I' => return .{ .key = KeyEvent.special(.focus_gained, .none) },
            'O' => return .{ .key = KeyEvent.special(.focus_lost, .none) },

            // Tilde (~) Komutları (Delete, PageUp, PageDown, Fn, Bracketed Paste)
            '~' => {
                switch (p1) {
                    1, 7 => return .{ .key = KeyEvent.special(.home, mod) },
                    2 => return .{ .key = KeyEvent.special(.insert, mod) },
                    3 => return .{ .key = KeyEvent.special(.delete, mod) },
                    4, 8 => return .{ .key = KeyEvent.special(.end, mod) },
                    5 => return .{ .key = KeyEvent.special(.page_up, mod) },
                    6 => return .{ .key = KeyEvent.special(.page_down, mod) },

                    11...15 => {
                        const fn_idx: u8 = @intCast(p1 - 10);
                        return .{ .key = KeyEvent.special(@enumFromInt(@intFromEnum(SpecialKey.f1) + fn_idx - 1), mod) };
                    },
                    17...21 => {
                        const fn_idx: u8 = @intCast(p1 - 11);
                        return .{ .key = KeyEvent.special(@enumFromInt(@intFromEnum(SpecialKey.f1) + fn_idx - 1), mod) };
                    },
                    23...24 => {
                        const fn_idx: u8 = @intCast(p1 - 12);
                        return .{ .key = KeyEvent.special(@enumFromInt(@intFromEnum(SpecialKey.f1) + fn_idx - 1), mod) };
                    },

                    // Bracketed Paste Başlangıcı
                    200 => {
                        self.in_paste = true;
                        self.paste_buffer.clearRetainingCapacity();
                        return .{ .key = KeyEvent.special(.paste_start, .none) };
                    },
                    201 => {
                        return .{ .key = KeyEvent.special(.paste_end, .none) };
                    },
                    else => {},
                }
            },
            else => {},
        }

        return null;
    }

    fn processSs3(self: *InputParser, byte: u8) ?InputEvent {
        defer self.reset();
        return switch (byte) {
            'P' => .{ .key = KeyEvent.special(.f1, .none) },
            'Q' => .{ .key = KeyEvent.special(.f2, .none) },
            'R' => .{ .key = KeyEvent.special(.f3, .none) },
            'S' => .{ .key = KeyEvent.special(.f4, .none) },
            'H' => .{ .key = KeyEvent.special(.home, .none) },
            'F' => .{ .key = KeyEvent.special(.end, .none) },
            else => null,
        };
    }

    fn processSgrMouse(self: *InputParser, byte: u8) ?InputEvent {
        if (byte >= '0' and byte <= '9') {
            self.current_param = self.current_param * 10 + (byte - '0');
            self.has_param = true;
            return null;
        }

        if (byte == ';') {
            self.commitParam();
            return null;
        }

        if (byte == 'M' or byte == 'm') {
            self.commitParam();
            defer self.reset();

            const b_code = if (self.param_count > 0) self.params[0] else 0;
            const px = if (self.param_count > 1) self.params[1] else 1;
            const py = if (self.param_count > 2) self.params[2] else 1;

            const is_release = (byte == 'm');
            const col = if (px > 0) px - 1 else 0;
            const row = if (py > 0) py - 1 else 0;

            var mod = Modifiers.none;
            if ((b_code & 4) != 0) mod.shift = true;
            if ((b_code & 8) != 0) mod.alt = true;
            if ((b_code & 16) != 0) mod.ctrl = true;

            const btn_base = b_code & 3;
            var button: MouseButton = .none;
            var action: MouseAction = if (is_release) .release else .press;

            if ((b_code & 64) != 0) {
                // Tekerlek
                button = if ((b_code & 1) != 0) .wheel_down else .wheel_up;
                action = .press;
            } else if ((b_code & 32) != 0) {
                // Sürükleme (Drag) veya Hareket
                action = .drag;
                button = switch (btn_base) {
                    0 => .left,
                    1 => .middle,
                    2 => .right,
                    else => .none,
                };
            } else {
                button = switch (btn_base) {
                    0 => .left,
                    1 => .middle,
                    2 => .right,
                    else => .none,
                };
            }

            return .{
                .mouse = .{
                    .col = col,
                    .row = row,
                    .button = button,
                    .action = action,
                    .modifiers = mod,
                },
            };
        }

        self.reset();
        return null;
    }

    fn processUtf8(self: *InputParser, byte: u8) ?InputEvent {
        if (self.utf8_len < self.utf8_buf.len) {
            self.utf8_buf[self.utf8_len] = byte;
            self.utf8_len += 1;
        }

        if (self.utf8_len >= self.utf8_expected) {
            defer self.reset();
            const slice = self.utf8_buf[0..self.utf8_len];
            const cp = std.unicode.utf8Decode(slice) catch return null;
            return .{ .key = KeyEvent.char(cp, .none) };
        }

        return null;
    }

    fn processPasteByte(self: *InputParser, byte: u8) !?InputEvent {
        // \x1b[201~ bitiş dizisini kontrol et
        const len = self.paste_buffer.items.len;
        if (byte == '~' and len >= 5) {
            const tail = self.paste_buffer.items[len - 5 .. len];
            if (std.mem.eql(u8, tail, "\x1b[201")) {
                self.paste_buffer.shrinkRetainingCapacity(len - 5);
                self.in_paste = false;
                self.reset();
                return .{ .paste = self.paste_buffer.items };
            }
        }

        try self.paste_buffer.append(self.allocator, byte);
        return null;
    }

    fn parseCsiModifier(p: u16) Modifiers {
        if (p == 0) return .none;
        const code = p - 1;
        return .{
            .shift = (code & 1) != 0,
            .alt = (code & 2) != 0,
            .ctrl = (code & 4) != 0,
            .meta = (code & 8) != 0,
        };
    }
};

// -----------------------------------------------------------------------------
// Unit Testler
// -----------------------------------------------------------------------------

test "input parser: basic ascii and ctrl keys" {
    var parser = InputParser.init(std.testing.allocator);
    defer parser.deinit();

    var events = std.ArrayList(InputEvent).empty;
    defer events.deinit(std.testing.allocator);

    try parser.parse("a", &events);
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.char('a', .none)));

    events.clearRetainingCapacity();
    try parser.parse("\x01", &events); // Ctrl+A
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.char('a', .{ .ctrl = true })));

    events.clearRetainingCapacity();
    try parser.parse("\x03", &events); // Ctrl+C
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.char('c', .{ .ctrl = true })));
}

test "input parser: alt keys and arrow keys" {
    var parser = InputParser.init(std.testing.allocator);
    defer parser.deinit();

    var events = std.ArrayList(InputEvent).empty;
    defer events.deinit(std.testing.allocator);

    try parser.parse("\x1bb", &events); // Alt+B
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.char('b', .{ .alt = true })));

    events.clearRetainingCapacity();
    try parser.parse("\x1b[A", &events); // Up Arrow
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.special(.up, .none)));

    events.clearRetainingCapacity();
    try parser.parse("\x1b[1;5A", &events); // Ctrl+Up Arrow
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.special(.up, .{ .ctrl = true })));
}

test "input parser: sgr 1006 mouse and utf8 multi-byte" {
    var parser = InputParser.init(std.testing.allocator);
    defer parser.deinit();

    var events = std.ArrayList(InputEvent).empty;
    defer events.deinit(std.testing.allocator);

    // SGR Mouse Left Click at (10, 20) -> col=9, row=19
    try parser.parse("\x1b[<0;10;20M", &events);
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    const m = events.items[0].mouse;
    try std.testing.expectEqual(@as(u16, 9), m.col);
    try std.testing.expectEqual(@as(u16, 19), m.row);
    try std.testing.expectEqual(MouseButton.left, m.button);
    try std.testing.expectEqual(MouseAction.press, m.action);

    // UTF-8 'ğ' (0xC4 0x9F)
    events.clearRetainingCapacity();
    try parser.parse("\xc4\x9f", &events);
    try std.testing.expectEqual(@as(usize, 1), events.items.len);
    try std.testing.expect(events.items[0].key.eql(KeyEvent.char(0x011F, .none)));
}
