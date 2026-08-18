//! Cancellation token (tasarım Bölüm 3.1).
//!
//! İptal bir kez tetiklenir; token sahipleri `isCancelled` ile iptali yield/IO
//! noktasında gözlemler (tasarım 3.2: cancellation yield veya IO noktasında
//! garanti edilir). Token tek yönlüdür: iptal geri alınamaz.
//! Üst token iptal edildiğinde alt token (child token) da otomatik olarak iptal kabul edilir.

const std = @import("std");

pub const CancellationToken = struct {
    cancelled: std.atomic.Value(bool) = std.atomic.Value(bool).init(false),
    parent: ?*const CancellationToken = null,

    pub fn init() CancellationToken {
        return .{};
    }

    /// Bir üst token'a bağlı alt token oluşturur. Üst token iptal edilirse,
    /// bu alt token da iptal edilmiş sayılır. Alt token bağımsız olarak da iptal edilebilir.
    pub fn initChild(parent: *const CancellationToken) CancellationToken {
        return .{
            .parent = parent,
        };
    }

    pub fn cancel(self: *CancellationToken) void {
        self.cancelled.store(true, .release);
    }

    pub fn isCancelled(self: *const CancellationToken) bool {
        if (self.cancelled.load(.acquire)) return true;
        if (self.parent) |p| {
            return p.isCancelled();
        }
        return false;
    }

    /// İptal edilmişse `error.Cancelled` döner; yield/IO noktalarında çağrılır.
    pub fn check(self: *const CancellationToken) CancelError!void {
        if (self.isCancelled()) return error.Cancelled;
    }
};

pub const CancelError = error{ Cancelled };

test "iptal baslangicta kapali" {
    var token = CancellationToken.init();
    try std.testing.expect(!token.isCancelled());
    try token.check();
}

test "iptal kalici ve tek yonlu" {
    var token = CancellationToken.init();
    token.cancel();
    try std.testing.expect(token.isCancelled());
    try std.testing.expectError(error.Cancelled, token.check());
}

test "child token ust token iptalini miras alir" {
    var parent = CancellationToken.init();
    const child = CancellationToken.initChild(&parent);

    try std.testing.expect(!child.isCancelled());
    parent.cancel();
    try std.testing.expect(child.isCancelled());
    try std.testing.expectError(error.Cancelled, child.check());
}

test "child token bagimsiz iptal edilebilir" {
    var parent = CancellationToken.init();
    var child = CancellationToken.initChild(&parent);

    child.cancel();
    try std.testing.expect(child.isCancelled());
    try std.testing.expect(!parent.isCancelled());
}
