//! omnitrix-tui: Unicode ve UTF-8 Genişlik / Hizalama Motoru (tasarım Bölüm 5.1, Doğrulama 11).
//!
//! Özellikler:
//! - UTF-8 geçerlilik ve güvenli çözümleme (multibyte kod noktalarını bölmez).
//! - Görsel sütun genişliği (strWidth / charWidth):
//!   - ASCII: 1 sütun
//!   - East Asian Wide / Fullwidth (CJK, Hiragana, Katakana, Hangul): 2 sütun
//!   - Emojiler: 2 sütun
//!   - Kontrol karakterleri & Zero-Width karakterler: 0 sütun
//! - Görsel sütun genişliğine göre güvenli kırpma (truncateToWidth) ve doldurma (padToWidth).
//! - Otomatik satır kaydırma (wrapText).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");

/// Tek bir Unicode kod noktasının terminal sütun genişliğini hesaplar.
pub fn codepointWidth(cp: u21) u8 {
    // Kontrol karakterleri ve null
    if (cp == 0 or (cp >= 0x0001 and cp <= 0x001F) or (cp >= 0x007F and cp <= 0x009F)) {
        return 0;
    }

    // Zero-width boşluklar, birleştirici işaretler (Combining Diacritical Marks), ZWJ, varyasyon seçiciler
    if (cp >= 0x0300 and cp <= 0x036F) return 0; // Combining Diacritical Marks
    if (cp >= 0x1AB0 and cp <= 0x1AFF) return 0; // Combining Diacritical Marks Extended
    if (cp >= 0x1DC0 and cp <= 0x1DFF) return 0; // Combining Diacritical Marks Supplement
    if (cp >= 0x200B and cp <= 0x200F) return 0; // Zero width space, ZWNJ, ZWJ, LRM, RLM
    if (cp >= 0x202A and cp <= 0x202E) return 0; // BiDi controls
    if (cp >= 0x2060 and cp <= 0x206F) return 0; // Invisible formatting
    if (cp >= 0xFE00 and cp <= 0xFE0F) return 0; // Variation Selectors
    if (cp >= 0xE0100 and cp <= 0xE01EF) return 0; // Variation Selectors Supplement

    // Emojiler ve semboller (Genişlik = 2)
    if (cp >= 0x1F300 and cp <= 0x1FAFF) return 2; // Emojis & Pictographs
    if (cp >= 0x1F600 and cp <= 0x1F64F) return 2; // Emoticons
    if (cp >= 0x1F680 and cp <= 0x1F6FF) return 2; // Transport & Map
    if (cp >= 0x2600 and cp <= 0x27BF) return 2; // Misc Symbols & Dingbats
    if (cp >= 0x2B50 and cp <= 0x2B55) return 2; // Stars / shapes

    // East Asian Wide / Fullwidth (CJK, Hangul, Fullwidth Forms - Genişlik = 2)
    if (cp >= 0x1100 and cp <= 0x115F) return 2; // Hangul Jamo
    if (cp >= 0x2E80 and cp <= 0x2EFF) return 2; // CJK Radicals Supplement
    if (cp >= 0x3000 and cp <= 0x303E) return 2; // CJK Symbols and Punctuation
    if (cp >= 0x3040 and cp <= 0x309F) return 2; // Hiragana
    if (cp >= 0x30A0 and cp <= 0x30FF) return 2; // Katakana
    if (cp >= 0x3130 and cp <= 0x318F) return 2; // Hangul Compatibility Jamo
    if (cp >= 0x3200 and cp <= 0x32FF) return 2; // Enclosed CJK Letters
    if (cp >= 0x3400 and cp <= 0x4DBF) return 2; // CJK Unified Ideographs Ext A
    if (cp >= 0x4E00 and cp <= 0x9FFF) return 2; // CJK Unified Ideographs
    if (cp >= 0xAC00 and cp <= 0xD7AF) return 2; // Hangul Syllables
    if (cp >= 0xF900 and cp <= 0xFAFF) return 2; // CJK Compatibility Ideographs
    if (cp >= 0xFF01 and cp <= 0xFF60) return 2; // Fullwidth Forms
    if (cp >= 0xFFE0 and cp <= 0xFFE6) return 2; // Fullwidth Signs
    if (cp >= 0x20000 and cp <= 0x2FFFD) return 2; // CJK Extensions B/C/D/E/F

    // Standart ASCII ve diğer tek sütunlu karakterler
    return 1;
}

/// UTF-8 dizisinin toplam görsel terminal sütun genişliğini hesaplar.
pub fn strWidth(text: []const u8) usize {
    var width: usize = 0;
    var i: usize = 0;
    while (i < text.len) {
        const len = std.unicode.utf8ByteSequenceLength(text[i]) catch 1;
        if (i + len > text.len) {
            width += 1;
            break;
        }
        const slice = text[i .. i + len];
        const cp = std.unicode.utf8Decode(slice) catch {
            width += 1;
            i += 1;
            continue;
        };
        width += codepointWidth(cp);
        i += len;
    }
    return width;
}

/// Metni verilen görsel sütun genişliğine göre güvenli bir şekilde keser (multibyte bölmez).
/// İsteğe bağlı olarak sonuna ellipsis ekler.
pub fn truncateToWidth(
    allocator: std.mem.Allocator,
    text: []const u8,
    max_width: usize,
    ellipsis: []const u8,
) ![]u8 {
    const total_w = strWidth(text);
    if (total_w <= max_width) {
        return try allocator.dupe(u8, text);
    }

    const ell_w = strWidth(ellipsis);
    const budget = if (max_width >= ell_w) max_width - ell_w else 0;

    var cur_w: usize = 0;
    var cut_byte_idx: usize = 0;
    var i: usize = 0;

    while (i < text.len) {
        const len = std.unicode.utf8ByteSequenceLength(text[i]) catch 1;
        if (i + len > text.len) break;
        const slice = text[i .. i + len];
        const cp = std.unicode.utf8Decode(slice) catch {
            i += 1;
            continue;
        };
        const w = codepointWidth(cp);
        if (cur_w + w > budget) {
            break;
        }
        cur_w += w;
        i += len;
        cut_byte_idx = i;
    }

    var result = std.ArrayList(u8).empty;
    errdefer result.deinit(allocator);

    try result.appendSlice(allocator, text[0..cut_byte_idx]);
    if (ellipsis.len > 0 and cur_w + ell_w <= max_width) {
        try result.appendSlice(allocator, ellipsis);
    }

    return try result.toOwnedSlice(allocator);
}

/// Metni verilen görsel genişliğe tamamlayacak kadar boşlukla doldurur.
pub fn padToWidth(
    allocator: std.mem.Allocator,
    text: []const u8,
    target_width: usize,
) ![]u8 {
    const cur_w = strWidth(text);
    if (cur_w >= target_width) {
        return try allocator.dupe(u8, text);
    }

    const padding = target_width - cur_w;
    const out = try allocator.alloc(u8, text.len + padding);
    @memcpy(out[0..text.len], text);
    @memset(out[text.len..], ' ');
    return out;
}

/// Metni verilen görsel sütun genişliğine göre satırlara böler (word wrap).
pub fn wrapText(
    allocator: std.mem.Allocator,
    text: []const u8,
    max_width: usize,
) !std.ArrayList([]const u8) {
    var lines = std.ArrayList([]const u8).empty;
    errdefer {
        for (lines.items) |l| allocator.free(l);
        lines.deinit(allocator);
    }

    if (max_width == 0) {
        try lines.append(allocator, try allocator.dupe(u8, ""));
        return lines;
    }

    var line_it = std.mem.splitScalar(u8, text, '\n');
    while (line_it.next()) |raw_line| {
        if (raw_line.len == 0) {
            try lines.append(allocator, try allocator.dupe(u8, ""));
            continue;
        }

        var cur_line = std.ArrayList(u8).empty;
        errdefer cur_line.deinit(allocator);
        var cur_width: usize = 0;

        var word_it = std.mem.splitScalar(u8, raw_line, ' ');
        var first_word = true;

        while (word_it.next()) |word| {
            const word_w = strWidth(word);
            const space_w: usize = if (first_word) 0 else 1;

            if (!first_word and cur_width + space_w + word_w > max_width) {
                // Mevcut satırı tamamla ve yeni satıra geç
                try lines.append(allocator, try cur_line.toOwnedSlice(allocator));
                cur_line = std.ArrayList(u8).empty;
                cur_width = 0;
                first_word = true;
            }

            if (word_w > max_width) {
                // Kelime tek satıra sığmıyor, karakter bazında böl
                if (!first_word) {
                    try cur_line.append(allocator, ' ');
                    cur_width += 1;
                }
                var byte_idx: usize = 0;
                while (byte_idx < word.len) {
                    const seq_len = std.unicode.utf8ByteSequenceLength(word[byte_idx]) catch 1;
                    if (byte_idx + seq_len > word.len) break;
                    const char_bytes = word[byte_idx .. byte_idx + seq_len];
                    const cp = std.unicode.utf8Decode(char_bytes) catch {
                        byte_idx += 1;
                        continue;
                    };
                    const cw = codepointWidth(cp);

                    if (cur_width + cw > max_width and cur_width > 0) {
                        try lines.append(allocator, try cur_line.toOwnedSlice(allocator));
                        cur_line = std.ArrayList(u8).empty;
                        cur_width = 0;
                    }

                    try cur_line.appendSlice(allocator, char_bytes);
                    cur_width += cw;
                    byte_idx += seq_len;
                }
                first_word = false;
            } else {
                if (!first_word) {
                    try cur_line.append(allocator, ' ');
                    cur_width += 1;
                }
                try cur_line.appendSlice(allocator, word);
                cur_width += word_w;
                first_word = false;
            }
        }

        if (cur_line.items.len > 0 or first_word) {
            try lines.append(allocator, try cur_line.toOwnedSlice(allocator));
        }
    }

    return lines;
}

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11)
// -----------------------------------------------------------------------------

test "unicode genislik hesaplama: ascii, cjk, emoji ve turkce" {
    try std.testing.expectEqual(@as(usize, 5), strWidth("hello"));
    try std.testing.expectEqual(@as(usize, 7), strWidth("merhaba"));
    try std.testing.expectEqual(@as(usize, 6), strWidth("türkçe")); // ü, ç tek sütun genişliğinde

    // CJK ideografları 2 sütun
    try std.testing.expectEqual(@as(usize, 4), strWidth("你好")); // 2 * 2 = 4
    try std.testing.expectEqual(@as(usize, 6), strWidth("日本語")); // 3 * 2 = 6

    // Emojiler 2 sütun
    try std.testing.expectEqual(@as(usize, 2), strWidth("🚀"));
    try std.testing.expectEqual(@as(usize, 4), strWidth("⚡🔥"));
}

test "unicode truncateToWidth: multibyte bolmez ve ellipsis ekler" {
    const text = "Omnitrix 🚀 Güçlü Runtime";
    const truncated = try truncateToWidth(std.testing.allocator, text, 14, "...");
    defer std.testing.allocator.free(truncated);

    try std.testing.expect(strWidth(truncated) <= 14);
    try std.testing.expect(std.mem.endsWith(u8, truncated, "..."));
}

test "unicode padToWidth: dogru sutuna tamamlar" {
    const text = "Agent ⚡"; // 6 + 2 = 8 görsel genişlik
    const padded = try padToWidth(std.testing.allocator, text, 12);
    defer std.testing.allocator.free(padded);

    try std.testing.expectEqual(@as(usize, 12), strWidth(padded));
    try std.testing.expectEqualStrings("Agent ⚡    ", padded);
}

test "unicode wrapText: coklu satira bolme" {
    const text = "Omnitrix tek süreç bir mimaridir. Performans ve güvenlik esastır.";
    var lines = try wrapText(std.testing.allocator, text, 20);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    try std.testing.expect(lines.items.len >= 3);
    for (lines.items) |l| {
        try std.testing.expect(strWidth(l) <= 20);
    }
}
