const std = @import("std");

/// Decode a single UTF-8 codepoint from a byte slice starting at index i.
/// Returns the codepoint and the number of bytes consumed.
pub fn decodeCodepoint(text: []const u8, i: usize) struct { cp: u21, len: u8 } {
    if (i >= text.len) return .{ .cp = 0, .len = 1 };
    const slice = text[i..];
    const cp = std.unicode.wtf8Decode(slice) catch return .{ .cp = text[i], .len = 1 };
    // Calculate byte length by checking leading byte
    const byte_len: u8 = if (text[i] < 0x80) 1
        else if (text[i] < 0xE0) 2
        else if (text[i] < 0xF0) 3
        else 4;
    return .{ .cp = cp, .len = byte_len };
}

/// Encode a codepoint to UTF-8, returning the bytes written.
pub fn encodeCodepoint(cp: u21, buf: *[4]u8) []const u8 {
    const len = std.unicode.wtf8Encode(cp, buf) catch {
        // Fallback: replace with replacement character
        buf[0] = 0xEF;
        buf[1] = 0xBF;
        buf[2] = 0xBD;
        return buf[0..3];
    };
    return buf[0..len];
}

/// Get byte length of a UTF-8 character at position i
pub fn byteLength(text: []const u8, i: usize) u8 {
    if (i >= text.len) return 1;
    if (text[i] < 0x80) return 1;
    if (text[i] < 0xE0) return 2;
    if (text[i] < 0xF0) return 3;
    return 4;
}
