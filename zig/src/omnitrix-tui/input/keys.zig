//! omnitrix-tui/input/keys.zig
//!
//! Terminal girdi modelleri: Tuşlar, Değiştiriciler (Modifiers), Fare ve Özel Olaylar.
//! Hiçbir harici kütüphane bağımlılığı yoktur; saf Zig ile sıfırdan tasarlanmıştır.

const std = @import("std");

/// Tuş değiştirici bayrakları (Ctrl, Alt, Shift vb.)
pub const Modifiers = packed struct(u8) {
    shift: bool = false,
    alt: bool = false,
    ctrl: bool = false,
    meta: bool = false,
    _reserved: u4 = 0,

    pub const none = Modifiers{};

    pub fn eql(self: Modifiers, other: Modifiers) bool {
        return @as(u8, @bitCast(self)) == @as(u8, @bitCast(other));
    }
};

/// Özel fonksiyon ve kontrol tuşları
pub const SpecialKey = enum {
    // Navigasyon
    up,
    down,
    left,
    right,
    home,
    end,
    page_up,
    page_down,

    // Düzenleme
    backspace,
    delete,
    insert,
    enter,
    tab,
    backtab, // Shift+Tab
    escape,

    // Fonksiyon Tuşları
    f1,
    f2,
    f3,
    f4,
    f5,
    f6,
    f7,
    f8,
    f9,
    f10,
    f11,
    f12,

    // Özel Terminal Olayları
    focus_gained,
    focus_lost,
    paste_start,
    paste_end,
};

/// Tuş kodu: Ya UTF-8 Unicode karakter ya da özel fonksiyon tuşu
pub const KeyCode = union(enum) {
    char: u21,
    special: SpecialKey,

    pub fn eql(self: KeyCode, other: KeyCode) bool {
        return switch (self) {
            .char => |c| switch (other) {
                .char => |oc| c == oc,
                else => false,
            },
            .special => |s| switch (other) {
                .special => |os| s == os,
                else => false,
            },
        };
    }
};

/// Tam Klavye Olayı
pub const KeyEvent = struct {
    code: KeyCode,
    modifiers: Modifiers = .none,

    pub fn char(c: u21, mod: Modifiers) KeyEvent {
        return .{ .code = .{ .char = c }, .modifiers = mod };
    }

    pub fn special(s: SpecialKey, mod: Modifiers) KeyEvent {
        return .{ .code = .{ .special = s }, .modifiers = mod };
    }

    pub fn eql(self: KeyEvent, other: KeyEvent) bool {
        return self.code.eql(other.code) and self.modifiers.eql(other.modifiers);
    }
};

/// Fare Butonu ve Eylemi
pub const MouseButton = enum {
    left,
    middle,
    right,
    wheel_up,
    wheel_down,
    none,
};

pub const MouseAction = enum {
    press,
    release,
    drag,
    move,
};

/// Fare Olayı (SGR 1006 Formatı)
pub const MouseEvent = struct {
    col: u16,
    row: u16,
    button: MouseButton,
    action: MouseAction,
    modifiers: Modifiers = .none,
};

/// Ayrıştırılmış Terminal Girdi Olayı
pub const InputEvent = union(enum) {
    key: KeyEvent,
    mouse: MouseEvent,
    paste: []const u8,
    resize: struct { cols: u16, rows: u16 },
    cursor_position: struct { col: u16, row: u16 },
    unknown: []const u8,
};

test "key event creation and equality" {
    const k1 = KeyEvent.char('c', .{ .ctrl = true });
    const k2 = KeyEvent.char('c', .{ .ctrl = true });
    const k3 = KeyEvent.char('c', .{ .alt = true });
    const k4 = KeyEvent.special(.enter, .none);

    try std.testing.expect(k1.eql(k2));
    try std.testing.expect(!k1.eql(k3));
    try std.testing.expect(!k1.eql(k4));
}
