//! Dosya içerik hash'i (tasarım Bölüm 6.1).
//!
//! Ledger dosya içeriği yerine hash, metadata ve bounded patch tutar (Bölüm 8).
//! `hashBytes` saf fonksiyondur; `hashFile` `std.Io` üzerinden dosyayı bounded
//! limit ile okur.

const std = @import("std");
const mutation = @import("mutation.zig");

pub const Hash = mutation.Hash;

pub const HashError = error{
    FileTooLarge,
    ReadFailed,
};

/// İçeriğin SHA-256 özeti (tasarım 6.1: işlemden önce ve sonra kesin hash).
pub fn hashBytes(data: []const u8) Hash {
    var out: Hash = undefined;
    std.crypto.hash.sha2.Sha256.hash(data, &out, .{});
    return out;
}

/// Dosyayı bounded limit ile okur ve hash'ler. `limit` aşımında
/// error.FileTooLarge (Bölüm 8: dosya içeriği yerine hash + bounded patch).
pub fn hashFile(
    dir: std.Io.Dir,
    io: std.Io,
    sub_path: []const u8,
    allocator: std.mem.Allocator,
    limit: usize,
) HashError!Hash {
    const data = dir.readFileAlloc(io, sub_path, allocator, .limited(limit)) catch |err| switch (err) {
        error.StreamTooLong => return error.FileTooLarge,
        else => return error.ReadFailed,
    };
    defer allocator.free(data);
    return hashBytes(data);
}

test "hashBytes deterministiktir" {
    const a = hashBytes("selam");
    const b = hashBytes("selam");
    try std.testing.expectEqualSlices(u8, &a, &b);
    const c = hashBytes("selam!");
    try std.testing.expect(!std.mem.eql(u8, &a, &c));
}

test "hashFile gercek dosyada calisir" {
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "x.txt", .data = "hello ledger" });
    const h = try hashFile(tmp.dir, std.testing.io, "x.txt", std.testing.allocator, 1024);
    try std.testing.expectEqualSlices(u8, &hashBytes("hello ledger"), &h);
}

test "hashFile limit asimini reddeder" {
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "big.txt", .data = "0123456789" });
    try std.testing.expectError(error.FileTooLarge, hashFile(tmp.dir, std.testing.io, "big.txt", std.testing.allocator, 4));
}
