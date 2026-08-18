//! omnitrix-ledger: Hunk diff hesaplama ve ayrıştırma (tasarım Bölüm 5.1, 6.2, Kol C - C4).
//!
//! Dosyalar arasındaki satır bazlı değişiklikleri hunk bloklarına ayırır.
//! İki aktörün (ör. kullanıcı ve ajan) aynı dosyanın farklı veya çakışan
//! satırlarına dokunup dokunmadığını belirler.
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");

pub const Hunk = struct {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,

    /// İki hunk'ın eski dosya satır aralıklarının çakışıp çakışmadığını kontrol eder.
    pub fn overlaps(self: Hunk, other: Hunk) bool {
        const self_end = self.old_start + self.old_count;
        const other_end = other.old_start + other.old_count;

        if (self_end <= other.old_start) return false;
        if (other_end <= self.old_start) return false;
        return true;
    }
};

/// Basit ve deterministik satır listesi ayrıştırıcı.
fn splitLines(allocator: std.mem.Allocator, text: []const u8) !std.ArrayList([]const u8) {
    var lines = std.ArrayList([]const u8).empty;
    errdefer lines.deinit(allocator);

    var it = std.mem.splitScalar(u8, text, '\n');
    while (it.next()) |line| {
        try lines.append(allocator, line);
    }
    return lines;
}

/// İki metin arasındaki satır farklarını tespit edip hunk listesi oluşturur.
pub fn computeHunks(
    allocator: std.mem.Allocator,
    old_text: []const u8,
    new_text: []const u8,
) !std.ArrayList(Hunk) {
    var hunks = std.ArrayList(Hunk).empty;
    errdefer hunks.deinit(allocator);

    var old_lines = try splitLines(allocator, old_text);
    defer old_lines.deinit(allocator);

    var new_lines = try splitLines(allocator, new_text);
    defer new_lines.deinit(allocator);

    const old_len = old_lines.items.len;
    const new_len = new_lines.items.len;

    var i: usize = 0;
    var j: usize = 0;

    while (i < old_len or j < new_len) {
        // Eşleşen satırları atla
        if (i < old_len and j < new_len and std.mem.eql(u8, old_lines.items[i], new_lines.items[j])) {
            i += 1;
            j += 1;
            continue;
        }

        // Fark başlangıcı
        const hunk_old_start = i + 1;
        const hunk_new_start = j + 1;
        var hunk_old_count: usize = 0;
        var hunk_new_count: usize = 0;

        // Değişen bloğu sınırla (bir sonraki eşleşen satıra kadar)
        while (i < old_len or j < new_len) {
            if (i < old_len and j < new_len and std.mem.eql(u8, old_lines.items[i], new_lines.items[j])) {
                break;
            }
            if (i < old_len) {
                i += 1;
                hunk_old_count += 1;
            }
            if (j < new_len) {
                j += 1;
                hunk_new_count += 1;
            }
        }

        try hunks.append(allocator, .{
            .old_start = hunk_old_start,
            .old_count = hunk_old_count,
            .new_start = hunk_new_start,
            .new_count = hunk_new_count,
        });
    }

    return hunks;
}

test "computeHunks ve overlaps testi" {
    const old_text = "line1\nline2\nline3\nline4\nline5";
    const new_text = "line1\nline2_mod\nline3\nline4_mod\nline5";

    var hunks = try computeHunks(std.testing.allocator, old_text, new_text);
    defer hunks.deinit(std.testing.allocator);

    try std.testing.expectEqual(@as(usize, 2), hunks.items.len);
    // 1. Hunk: line2 -> line2_mod
    try std.testing.expectEqual(@as(usize, 2), hunks.items[0].old_start);
    try std.testing.expectEqual(@as(usize, 1), hunks.items[0].old_count);

    // 2. Hunk: line4 -> line4_mod
    try std.testing.expectEqual(@as(usize, 4), hunks.items[1].old_start);
    try std.testing.expectEqual(@as(usize, 1), hunks.items[1].old_count);

    // Çakışmama kontrolü
    try std.testing.expect(!hunks.items[0].overlaps(hunks.items[1]));

    // Çakışan iki hunk testi
    const h1 = Hunk{ .old_start = 5, .old_count = 10, .new_start = 5, .new_count = 12 };
    const h2 = Hunk{ .old_start = 12, .old_count = 5, .new_start = 14, .new_count = 5 };
    const h3 = Hunk{ .old_start = 20, .old_count = 5, .new_start = 25, .new_count = 5 };

    try std.testing.expect(h1.overlaps(h2));
    try std.testing.expect(!h1.overlaps(h3));
}
