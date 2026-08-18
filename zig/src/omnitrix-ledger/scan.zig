//! Proje kökü taraması (tasarım Bölüm 5.2).
//!
//! Varsayılan davranış Git'e bağlı değildir. Hariç tutulanlar:
//! - VCS iç metadata klasörleri (`.git/`, `.hg/`, `.svn/`)
//! - Omnitrix'in kendi cache/build klasörleri (zig-cache, .zig-cache, zig-out, target)
//! - symlink ile proje kökünün dışına çıkan yollar (symlink girilmez)
//!
//! Proje içindeki normal ignored dosyalar DAHİL edilir (Bölüm 5.2).

const std = @import("std");

/// Hariç tutulacak üst seviye klasör adları (VCS metadata + Omnitrix cache/build).
pub const excluded_dir_names = [_][]const u8{
    ".git",
    ".hg",
    ".svn",
    "zig-cache",
    ".zig-cache",
    "zig-out",
    "target",
};

pub const ScanOptions = struct {
    /// Taramaya dahil edilecek maksimum dosya sayısı (bounded scan, Bölüm 8).
    max_entries: usize = 10_000,
};

pub const ScanError = std.Io.Dir.SelectiveWalker.Error ||
    @typeInfo(@typeInfo(@TypeOf(std.Io.Dir.SelectiveWalker.enter)).@"fn".return_type.?).error_union.error_set || error{
    TooManyEntries,
    WalkFailed,
} || std.mem.Allocator.Error;

pub const ScanResult = struct {
    /// Proje köküne göreli dosya yolları (sahiplenilmiş).
    files: std.ArrayList([]const u8),
    allocator: std.mem.Allocator,
    /// Atlanan (hariç tutulan) klasör sayısı.
    skipped_dirs: usize = 0,

    pub fn deinit(self: *ScanResult) void {
        for (self.files.items) |f| self.allocator.free(f);
        self.files.deinit(self.allocator);
        self.* = undefined;
    }
};

/// Klasör adı VCS metadata veya Omnitrix cache/build ise true (Bölüm 5.2).
pub fn isExcludedDirName(name: []const u8) bool {
    for (excluded_dir_names) |excluded| {
        if (std.mem.eql(u8, name, excluded)) return true;
    }
    return false;
}

/// Proje kökünü dolaşır; hariç tutulan klasörlere girmez, symlink'leri izlemez,
/// ignored dosyaları dahil eder. Dönen yollar göreli ve sahiplenilmiştir.
pub fn scanProject(
    dir: std.Io.Dir,
    io: std.Io,
    allocator: std.mem.Allocator,
    options: ScanOptions,
) ScanError!ScanResult {
    var result = ScanResult{
        .files = std.ArrayList([]const u8).empty,
        .allocator = allocator,
    };
    errdefer result.deinit();

    var walker = std.Io.Dir.walkSelectively(dir, allocator) catch |err| switch (err) {
        else => return error.WalkFailed,
    };
    defer walker.deinit();

    while (try walker.next(io)) |entry| {
        if (result.files.items.len >= options.max_entries) {
            return error.TooManyEntries;
        }
        switch (entry.kind) {
            .directory => {
                // Hariç tutulan klasöre girme; aksi halde içine in.
                if (isExcludedDirName(entry.basename)) {
                    result.skipped_dirs += 1;
                    continue;
                }
                try walker.enter(io, entry);
            },
            .sym_link => {
                // Symlink izlenmez: proje kökünün dışına çıkma hariç tutulur (5.2).
                continue;
            },
            .file => {
                const path = try allocator.dupe(u8, entry.path);
                errdefer allocator.free(path);
                try result.files.append(allocator, path);
            },
            else => {},
        }
    }

    return result;
}

test "excluded dir adlari dogru taninir" {
    try std.testing.expect(isExcludedDirName(".git"));
    try std.testing.expect(isExcludedDirName("zig-cache"));
    try std.testing.expect(isExcludedDirName(".zig-cache"));
    try std.testing.expect(isExcludedDirName("target"));
    try std.testing.expect(!isExcludedDirName("src"));
    try std.testing.expect(!isExcludedDirName("README.md"));
}

test "scan .git ve cache klasorlerini atlar, ignored dosyayi dahil eder" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();

    try tmp.dir.createDirPath(std.testing.io, "src");
    try tmp.dir.createDirPath(std.testing.io, ".git");
    try tmp.dir.createDirPath(std.testing.io, "zig-cache/o");
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "src/main.zig", .data = "fn main() {}" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = ".git/HEAD", .data = "ref" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "zig-cache/o/abc", .data = "cache" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = ".gitignore", .data = "secret.txt\n" });
    // Gitignore'da olmasına rağmen normal ignored dosya DAHİL edilir (5.2).
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "secret.txt", .data = "gizli" });

    var res = try scanProject(tmp.dir, std.testing.io, std.testing.allocator, .{});
    defer res.deinit();

    var has_main = false;
    var has_secret = false;
    var has_git_head = false;
    var has_cache = false;
    for (res.files.items) |f| {
        if (std.mem.eql(u8, f, "src/main.zig")) has_main = true;
        if (std.mem.eql(u8, f, "secret.txt")) has_secret = true;
        if (std.mem.eql(u8, f, ".git/HEAD")) has_git_head = true;
        if (std.mem.eql(u8, f, "zig-cache/o/abc")) has_cache = true;
    }
    try std.testing.expect(has_main);
    try std.testing.expect(has_secret); // ignored ama dahil
    try std.testing.expect(!has_git_head); // VCS metadata hariç
    try std.testing.expect(!has_cache); // Omnitrix cache hariç
    try std.testing.expect(res.skipped_dirs >= 2);
}

test "scan maksimum entry sinirini zorlar" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    for (0..5) |i| {
        var buf: [32]u8 = undefined;
        const name = try std.fmt.bufPrint(&buf, "f{d}.txt", .{i});
        try tmp.dir.writeFile(std.testing.io, .{ .sub_path = name, .data = "x" });
    }
    try std.testing.expectError(
        error.TooManyEntries,
        scanProject(tmp.dir, std.testing.io, std.testing.allocator, .{ .max_entries = 3 }),
    );
}
