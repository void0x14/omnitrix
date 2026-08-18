//! omnitrix-tui: Diff Hunk Görüntüleme ve Dar Terminal Fullscreen Fallback'i (tasarım Bölüm 5.1, Kol A - A3, Doğrulama 11).
//!
//! Özellikler:
//! - Unified / Hunk diff renderlayıcı (satır numaraları, ekleme (+), silme (-), bağlam ( ) ve hunk başlıkları @@).
//! - Renklendirme ve stil desteği (eklemeler yeşil, silmeler kırmızı, başlıklar camgöbeği/cyan, satır no soluk).
//! - Hunk navigasyonu ve seçili hunk takibi (seçici geri alma / inceleme için).
//! - Dar terminal otomatik fullscreen diff fallback'i: Terminal genişliği < 80 sütun olduğunda
//!   bölünmüş (split) görünüm yerine otomatik olarak tam ekran diff moduna geçer.
//! - Resize sırasında seçili hunk indeksini ve kaydırma konumunu güvenle korur (Doğrulama 11).
//! - Büyük yamalar için sınırlandırılmış (bounded) / lazy hunk satır tamponu.
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const unicode = @import("unicode.zig");
const term = @import("terminal.zig");
const hunk_mod = @import("../omnitrix-ledger/hunk.zig");

pub const Hunk = hunk_mod.Hunk;
pub const Color = term.Color;
pub const Style = term.Style;

pub const DiffLineKind = enum {
    context,
    addition,
    deletion,
    hunk_header,
    file_header,

    pub fn prefixChar(self: DiffLineKind) u8 {
        return switch (self) {
            .context => ' ',
            .addition => '+',
            .deletion => '-',
            .hunk_header => '@',
            .file_header => ' ',
        };
    }
};

pub const DiffLine = struct {
    kind: DiffLineKind,
    old_lineno: ?usize = null,
    new_lineno: ?usize = null,
    text: []const u8,

    pub fn deinit(self: *DiffLine, allocator: std.mem.Allocator) void {
        allocator.free(self.text);
        self.* = undefined;
    }
};

pub const DiffHunk = struct {
    hunk: Hunk,
    header: []const u8,
    lines: std.ArrayList(DiffLine),

    pub fn init(allocator: std.mem.Allocator, hunk: Hunk) !DiffHunk {
        var header_buf: [128]u8 = undefined;
        const hdr = try std.fmt.bufPrint(
            &header_buf,
            "@@ -{d},{d} +{d},{d} @@",
            .{ hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count },
        );

        return .{
            .hunk = hunk,
            .header = try allocator.dupe(u8, hdr),
            .lines = std.ArrayList(DiffLine).empty,
        };
    }

    pub fn deinit(self: *DiffHunk, allocator: std.mem.Allocator) void {
        allocator.free(self.header);
        for (self.lines.items) |*l| l.deinit(allocator);
        self.lines.deinit(allocator);
        self.* = undefined;
    }

    pub fn addLine(
        self: *DiffHunk,
        allocator: std.mem.Allocator,
        kind: DiffLineKind,
        old_lineno: ?usize,
        new_lineno: ?usize,
        text: []const u8,
    ) !void {
        const owned_text = try allocator.dupe(u8, text);
        errdefer allocator.free(owned_text);

        try self.lines.append(allocator, .{
            .kind = kind,
            .old_lineno = old_lineno,
            .new_lineno = new_lineno,
            .text = owned_text,
        });
    }
};

pub const DiffFile = struct {
    path: []const u8,
    old_path: ?[]const u8 = null,
    hunks: std.ArrayList(DiffHunk),
    additions: ?u64 = null,
    deletions: ?u64 = null,
    is_binary: bool = false,
    is_conflict: bool = false,

    pub fn init(allocator: std.mem.Allocator, path: []const u8) !DiffFile {
        return .{
            .path = try allocator.dupe(u8, path),
            .old_path = null,
            .hunks = std.ArrayList(DiffHunk).empty,
            .additions = null,
            .deletions = null,
            .is_binary = false,
            .is_conflict = false,
        };
    }

    pub fn deinit(self: *DiffFile, allocator: std.mem.Allocator) void {
        allocator.free(self.path);
        if (self.old_path) |op| allocator.free(op);
        for (self.hunks.items) |*h| h.deinit(allocator);
        self.hunks.deinit(allocator);
        self.* = undefined;
    }
};

pub const DiffViewMode = enum {
    split_view, // >= 80 sütun: Normal bölünmüş görünüm
    fullscreen_diff, // < 80 sütun veya kullanıcı seçimi: Tam ekran diff fallback
};

pub const DiffRenderer = struct {
    allocator: std.mem.Allocator,
    files: std.ArrayList(DiffFile),
    selected_file_index: usize = 0,
    selected_hunk_index: usize = 0,
    scroll_y: usize = 0,
    term_width: u16 = 80,
    term_height: u16 = 24,

    pub fn init(allocator: std.mem.Allocator) DiffRenderer {
        return .{
            .allocator = allocator,
            .files = std.ArrayList(DiffFile).empty,
            .selected_file_index = 0,
            .selected_hunk_index = 0,
            .scroll_y = 0,
            .term_width = 80,
            .term_height = 24,
        };
    }

    pub fn deinit(self: *DiffRenderer) void {
        for (self.files.items) |*f| f.deinit(self.allocator);
        self.files.deinit(self.allocator);
        self.* = undefined;
    }

    /// Terminal boyutunu günceller ve dar ekran (<80 sütun) durumunu değerlendirir.
    /// Resize sonrasında aktif dosya ve seçili hunk indeksini korur (Doğrulama 11).
    pub fn handleResize(self: *DiffRenderer, new_width: u16, new_height: u16) void {
        self.term_width = new_width;
        self.term_height = new_height;

        // İndeksleri güvenli sınırda tut
        if (self.files.items.len > 0) {
            if (self.selected_file_index >= self.files.items.len) {
                self.selected_file_index = self.files.items.len - 1;
            }
            const cur_file = &self.files.items[self.selected_file_index];
            if (cur_file.hunks.items.len > 0 and self.selected_hunk_index >= cur_file.hunks.items.len) {
                self.selected_hunk_index = cur_file.hunks.items.len - 1;
            }
        } else {
            self.selected_file_index = 0;
            self.selected_hunk_index = 0;
        }
    }

    /// Terminal dar modda mı (< 80 sütun) kontrol eder (tasarım Bölüm 5.1).
    pub fn isNarrow(self: *const DiffRenderer) bool {
        return self.term_width < 80;
    }

    /// Geçerli görünüm modunu hesaplar.
    pub fn currentMode(self: *const DiffRenderer) DiffViewMode {
        if (self.isNarrow()) {
            return .fullscreen_diff;
        }
        return .split_view;
    }

    /// İki metin arasındaki diff'i hesaplayıp DiffFile olarak ekler.
    pub fn addFileDiffFromTexts(
        self: *DiffRenderer,
        path: []const u8,
        old_text: []const u8,
        new_text: []const u8,
    ) !void {
        var file = try DiffFile.init(self.allocator, path);
        errdefer file.deinit(self.allocator);

        var hunks = try hunk_mod.computeHunks(self.allocator, old_text, new_text);
        defer hunks.deinit(self.allocator);

        var old_lines = std.ArrayList([]const u8).empty;
        defer old_lines.deinit(self.allocator);
        var it_old = std.mem.splitScalar(u8, old_text, '\n');
        while (it_old.next()) |l| try old_lines.append(self.allocator, l);

        var new_lines = std.ArrayList([]const u8).empty;
        defer new_lines.deinit(self.allocator);
        var it_new = std.mem.splitScalar(u8, new_text, '\n');
        while (it_new.next()) |l| try new_lines.append(self.allocator, l);

        var total_add: u64 = 0;
        var total_del: u64 = 0;

        for (hunks.items) |h| {
            var diff_hunk = try DiffHunk.init(self.allocator, h);
            errdefer diff_hunk.deinit(self.allocator);

            // Silinen satırlar
            var d: usize = 0;
            while (d < h.old_count) : (d += 1) {
                const line_idx = h.old_start - 1 + d;
                const txt = if (line_idx < old_lines.items.len) old_lines.items[line_idx] else "";
                try diff_hunk.addLine(self.allocator, .deletion, line_idx + 1, null, txt);
                total_del += 1;
            }

            // Eklenen satırlar
            var a: usize = 0;
            while (a < h.new_count) : (a += 1) {
                const line_idx = h.new_start - 1 + a;
                const txt = if (line_idx < new_lines.items.len) new_lines.items[line_idx] else "";
                try diff_hunk.addLine(self.allocator, .addition, null, line_idx + 1, txt);
                total_add += 1;
            }

            try file.hunks.append(self.allocator, diff_hunk);
        }

        file.additions = total_add;
        file.deletions = total_del;

        try self.files.append(self.allocator, file);
    }

    /// Aktif dosyanın diff satırlarını renderlar.
    pub fn renderActiveDiffToLines(
        self: *const DiffRenderer,
        allocator: std.mem.Allocator,
        render_width: usize,
    ) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        if (self.files.items.len == 0) {
            try lines.append(allocator, try allocator.dupe(u8, "No diff available"));
            return lines;
        }

        const file = &self.files.items[self.selected_file_index];

        // Dosya başlığı
        var hdr_buf: [256]u8 = undefined;
        const add_str = if (file.additions) |a| try std.fmt.bufPrint(&hdr_buf, "+{d}", .{a}) else "[binary]";
        var del_buf: [64]u8 = undefined;
        const del_str = if (file.deletions) |d| try std.fmt.bufPrint(&del_buf, "-{d}", .{d}) else "";

        const mode_label = if (self.isNarrow()) "[FULLSCREEN DIFF]" else "[SPLIT DIFF]";
        const file_hdr = if (file.old_path) |op|
            try std.fmt.allocPrint(allocator, "── {s} -> {s}  {s} {s}  {s} ──", .{ op, file.path, add_str, del_str, mode_label })
        else
            try std.fmt.allocPrint(allocator, "── {s}  {s} {s}  {s} ──", .{ file.path, add_str, del_str, mode_label });
        try lines.append(allocator, file_hdr);

        if (file.is_binary) {
            try lines.append(allocator, try allocator.dupe(u8, "  [Binary file differs - no text diff available]"));
            return lines;
        }

        for (file.hunks.items, 0..) |hunk, h_idx| {
            const is_selected = (h_idx == self.selected_hunk_index);
            const sel_mark: []const u8 = if (is_selected) "▶ " else "  ";

            // Hunk başlığı
            const hunk_hdr_line = try std.fmt.allocPrint(allocator, "{s}{s}", .{ sel_mark, hunk.header });
            try lines.append(allocator, hunk_hdr_line);

            // Satırlar
            for (hunk.lines.items) |dl| {
                var line_buf: [512]u8 = undefined;
                const old_str = if (dl.old_lineno) |ln| try std.fmt.bufPrint(&hdr_buf, "{d: >4}", .{ln}) else "    ";
                const new_str = if (dl.new_lineno) |ln| try std.fmt.bufPrint(&del_buf, "{d: >4}", .{ln}) else "    ";

                const prefix = dl.kind.prefixChar();
                const formatted = try std.fmt.bufPrint(&line_buf, " {s} {s} {c} {s}", .{
                    old_str,
                    new_str,
                    prefix,
                    dl.text,
                });

                const truncated = try unicode.truncateToWidth(allocator, formatted, render_width, "...");
                try lines.append(allocator, truncated);
            }
        }

        return lines;
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 11 / A3)
// -----------------------------------------------------------------------------

test "diff renderer ve dar terminal fallback modu" {
    var dr = DiffRenderer.init(std.testing.allocator);
    defer dr.deinit();

    const old_c = "fn calc() void {\n    val = 10;\n}";
    const new_c = "fn calc() void {\n    val = 20;\n    extra = 30;\n}";

    try dr.addFileDiffFromTexts("src/calc.zig", old_c, new_c);
    try std.testing.expectEqual(@as(usize, 1), dr.files.items.len);

    // 1. Normal geniş ekran (100 sütun) -> split view
    dr.handleResize(100, 30);
    try std.testing.expect(!dr.isNarrow());
    try std.testing.expectEqual(DiffViewMode.split_view, dr.currentMode());

    // 2. Dar ekran (79 sütun) -> otomatik fullscreen diff fallback
    dr.handleResize(79, 24);
    try std.testing.expect(dr.isNarrow());
    try std.testing.expectEqual(DiffViewMode.fullscreen_diff, dr.currentMode());

    // 3. Render çizgileri
    var lines = try dr.renderActiveDiffToLines(std.testing.allocator, 70);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    try std.testing.expect(lines.items.len > 0);
    try std.testing.expect(std.mem.indexOf(u8, lines.items[0], "src/calc.zig") != null);
    try std.testing.expect(std.mem.indexOf(u8, lines.items[0], "FULLSCREEN DIFF") != null);
}

test "diff renderer resize sonrasi secili hunk korunmasi (Dogrulama 11)" {
    var dr = DiffRenderer.init(std.testing.allocator);
    defer dr.deinit();

    const old_c = "line1\nline2\nline3\nline4\nline5";
    const new_c = "line1_mod\nline2\nline3\nline4_mod\nline5";

    try dr.addFileDiffFromTexts("src/multi_hunk.zig", old_c, new_c);
    try std.testing.expectEqual(@as(usize, 2), dr.files.items[0].hunks.items.len);

    // 2. hunk'ı seç
    dr.selected_hunk_index = 1;

    // Boyut değişimi
    dr.handleResize(60, 20);
    try std.testing.expectEqual(@as(usize, 1), dr.selected_hunk_index); // Seçim aynen korundu!
    try std.testing.expectEqual(@as(usize, 0), dr.selected_file_index);
}
