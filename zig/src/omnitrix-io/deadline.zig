//! Monotonic deadline (tasarım Bölüm 3.1).
//!
//! - monotonic saat: geriye sıçramaz, NTP'den etkilenmez
//! - deadline bir zaman noktasıdır; `remaining` <= 0 ise süre dolmuştur
//! - bağımlılık enjeksiyonu: testlerde gerçek saate ihtiyaç yok
//! - üretim yolunda `catch unreachable` veya `@panic` YOKTUR (I6 disiplini).

const std = @import("std");
const builtin = @import("builtin");

/// Monotonic saatten nanosaniye okur.
pub const Monotonic = struct {
    pub fn now() i128 {
        switch (builtin.os.tag) {
            .linux => {
                var ts: std.os.linux.timespec = undefined;
                const rc = std.os.linux.clock_gettime(.MONOTONIC, &ts);
                if (std.os.linux.errno(rc) != .SUCCESS) {
                    return 0;
                }
                return @as(i128, ts.sec) * std.time.ns_per_s + ts.nsec;
            },
            else => {
                var ts: std.posix.timespec = undefined;
                std.posix.clock_gettime(.MONOTONIC, &ts) catch return 0;
                return @as(i128, ts.sec) * std.time.ns_per_s + ts.nsec;
            },
        }
    }
};

/// Belirli bir zaman noktasına kadar süreli işlem.
pub const Deadline = struct {
    /// Mutlak monotonic nanosaniye.
    deadline_ns: i128,
    /// Testlerde sahte saat verilebilir; üretimde `Monotonic.now`.
    now_fn: *const fn () i128 = Monotonic.now,

    pub const infinity: Deadline = .{
        .deadline_ns = std.math.maxInt(i128),
        .now_fn = Monotonic.now,
    };

    pub fn fromNanos(now_fn: *const fn () i128, timeout_ns: i128) Deadline {
        if (timeout_ns <= 0) {
            return .{ .deadline_ns = now_fn(), .now_fn = now_fn };
        }
        const cur = now_fn();
        const dl = std.math.add(i128, cur, timeout_ns) catch std.math.maxInt(i128);
        return .{ .deadline_ns = dl, .now_fn = now_fn };
    }

    pub fn fromMillis(now_fn: *const fn () i128, timeout_ms: i128) Deadline {
        if (timeout_ms <= 0) {
            return .{ .deadline_ns = now_fn(), .now_fn = now_fn };
        }
        const timeout_ns = timeout_ms * std.time.ns_per_ms;
        return fromNanos(now_fn, timeout_ns);
    }

    pub fn fromSeconds(now_fn: *const fn () i128, timeout_sec: i128) Deadline {
        if (timeout_sec <= 0) {
            return .{ .deadline_ns = now_fn(), .now_fn = now_fn };
        }
        const timeout_ns = timeout_sec * std.time.ns_per_s;
        return fromNanos(now_fn, timeout_ns);
    }

    /// Kalan süre; <= 0 ise süre dolmuş.
    pub fn remaining(self: Deadline) i128 {
        if (self.deadline_ns == std.math.maxInt(i128)) {
            return std.math.maxInt(i128);
        }
        return self.deadline_ns - self.now_fn();
    }

    pub fn remainingMillis(self: Deadline) i64 {
        const rem = self.remaining();
        if (rem <= 0) return 0;
        if (rem == std.math.maxInt(i128)) return std.math.maxInt(i64);
        const ms = @divTrunc(rem, std.time.ns_per_ms);
        if (ms > std.math.maxInt(i64)) return std.math.maxInt(i64);
        return @intCast(ms);
    }

    pub fn expired(self: Deadline) bool {
        return self.remaining() <= 0;
    }

    pub fn isInfinite(self: Deadline) bool {
        return self.deadline_ns == std.math.maxInt(i128);
    }

    /// Süre dolmuşsa `error.DeadlineExceeded`, değilse void.
    pub fn check(self: Deadline) DeadlineError!void {
        if (self.expired()) return error.DeadlineExceeded;
    }
};

pub const DeadlineError = error{ DeadlineExceeded };

test "deadline remaining azalir ve expire olur" {
    const FakeClock = struct {
        var now_val: i128 = 1_000_000;
        fn f() i128 {
            return now_val;
        }
    };
    var d = Deadline.fromMillis(FakeClock.f, 100);
    try std.testing.expect(!d.expired());
    try std.testing.expect(d.remainingMillis() == 100);
    FakeClock.now_val += 100 * std.time.ns_per_ms;
    try std.testing.expect(d.expired());
    try std.testing.expect(d.remainingMillis() == 0);
    try std.testing.expectError(error.DeadlineExceeded, d.check());
}

test "deadline sifir ve negatif sure aninda expire" {
    const ZeroClock = struct {
        fn f() i128 {
            return 500;
        }
    };
    const d0 = Deadline.fromMillis(ZeroClock.f, 0);
    try std.testing.expect(d0.expired());
    const d_neg = Deadline.fromMillis(ZeroClock.f, -10);
    try std.testing.expect(d_neg.expired());
    const d_sec = Deadline.fromSeconds(ZeroClock.f, 0);
    try std.testing.expect(d_sec.expired());
}

test "monotonic saat ileri gider" {
    const a = Monotonic.now();
    const b = Monotonic.now();
    try std.testing.expect(b >= a);
}

test "deadline infinity asla expire olmaz" {
    const d = Deadline.infinity;
    try std.testing.expect(d.isInfinite());
    try std.testing.expect(!d.expired());
    try d.check();
}
