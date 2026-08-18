//! omnitrix-tui: Grok CLI Stili Sesli Mod (Voice Mode & Audio Equalizer)
//!
//! Özellikler:
//! - Canlı ses dinleme durumu (Listening / Standby).
//! - Canlı ses dalgası animatörü (Visual Audio Equalizer:  ▂▃▅▆▇▆▅▃ ).
//! - Grok tarzı ses giriş banner'ı ve transkripsiyon tamponu.

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");
const Color = term.Color;

pub const VoiceState = enum {
    off,
    listening,
    processing,
    speaking,
};

pub const VoiceMode = struct {
    allocator: std.mem.Allocator,
    state: VoiceState = .off,
    frame_counter: usize = 0,
    transcript: std.ArrayList(u8),

    const waveforms = [_][]const u8{
        " ▂▃▅▆▇▆▅▃ ",
        "▂▃▅▆▇█▇▆▅▃",
        "▃▅▆▇█▇▆▅▃ ",
        "▅▆▇█▇▆▅▃▂ ",
        "▆▇█▇▆▅▃▂  ",
        "▇█▇▆▅▃▂ ▂▃",
        "█▇▆▅▃▂ ▂▃▅",
        "▇▆▅▃▂ ▂▃▅▆",
    };

    pub fn init(allocator: std.mem.Allocator) VoiceMode {
        return .{
            .allocator = allocator,
            .state = .off,
            .frame_counter = 0,
            .transcript = std.ArrayList(u8).empty,
        };
    }

    pub fn deinit(self: *VoiceMode) void {
        self.transcript.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn toggle(self: *VoiceMode) void {
        self.state = if (self.state == .off) .listening else .off;
    }

    pub fn tick(self: *VoiceMode) void {
        if (self.state != .off) {
            self.frame_counter = (self.frame_counter + 1) % waveforms.len;
        }
    }

    pub fn currentWaveform(self: *const VoiceMode) []const u8 {
        return waveforms[self.frame_counter % waveforms.len];
    }

    /// Grok tarzı canlı sesli mod durum kutusunu render eder.
    pub fn renderVoiceBanner(self: *const VoiceMode, allocator: std.mem.Allocator, width: usize) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        if (self.state == .off or width < 30) return lines;

        // Üst kenarlık
        var top_buf = std.ArrayList(u8).empty;
        defer top_buf.deinit(allocator);
        try term.appendStyle(&top_buf, allocator, .{ .fg = Color{ .ansi = 117 }, .bold = true });
        try top_buf.appendSlice(allocator, theme.Box.top_left);
        try top_buf.appendSlice(allocator, " 🎙️ GROK VOICE MODE [ACTIVE] ");
        var pad: usize = if (width > 32) width - 32 else 1;
        while (pad > 0) : (pad -= 1) {
            try top_buf.appendSlice(allocator, theme.Box.horizontal);
        }
        try top_buf.appendSlice(allocator, theme.Box.top_right);
        try top_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try top_buf.toOwnedSlice(allocator));

        // Gövde (Equalizer animasyonu)
        var body_buf = std.ArrayList(u8).empty;
        defer body_buf.deinit(allocator);
        try term.appendStyle(&body_buf, allocator, .{ .fg = Color{ .ansi = 117 } });
        try body_buf.appendSlice(allocator, theme.Box.vertical);
        try body_buf.appendSlice(allocator, "  ");

        try term.appendStyle(&body_buf, allocator, .{ .fg = theme.Theme.text_green, .bold = true });
        try body_buf.appendSlice(allocator, self.currentWaveform());
        try term.appendStyle(&body_buf, allocator, .{ .fg = theme.Theme.text_main, .bold = true });
        try body_buf.appendSlice(allocator, " Listening... Speak now (Ctrl+V to toggle off)");

        const text_w = 12 + unicode.strWidth(" Listening... Speak now (Ctrl+V to toggle off)");
        var b_pad = if (width > text_w + 2) width - text_w - 2 else 1;
        while (b_pad > 0) : (b_pad -= 1) {
            try body_buf.appendSlice(allocator, " ");
        }
        try term.appendStyle(&body_buf, allocator, .{ .fg = Color{ .ansi = 117 } });
        try body_buf.appendSlice(allocator, theme.Box.vertical);
        try body_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try body_buf.toOwnedSlice(allocator));

        // Alt kenarlık
        var bot_buf = std.ArrayList(u8).empty;
        defer bot_buf.deinit(allocator);
        try term.appendStyle(&bot_buf, allocator, .{ .fg = Color{ .ansi = 117 } });
        try bot_buf.appendSlice(allocator, theme.Box.bottom_left);
        var b_pad2: usize = if (width > 2) width - 2 else 1;
        while (b_pad2 > 0) : (b_pad2 -= 1) {
            try bot_buf.appendSlice(allocator, theme.Box.horizontal);
        }
        try bot_buf.appendSlice(allocator, theme.Box.bottom_right);
        try bot_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try bot_buf.toOwnedSlice(allocator));

        return lines;
    }
};

test "voice mode toggle and waveform animation" {
    var voice = VoiceMode.init(std.testing.allocator);
    defer voice.deinit();

    try std.testing.expect(voice.state == .off);
    voice.toggle();
    try std.testing.expect(voice.state == .listening);

    const w1 = voice.currentWaveform();
    voice.tick();
    const w2 = voice.currentWaveform();
    try std.testing.expect(!std.mem.eql(u8, w1, w2));
}
