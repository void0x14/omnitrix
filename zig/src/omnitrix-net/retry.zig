//! Provider Timeout ve Üstel/Bounded Retry Politikası (tasarım Bölüm 3.3, 8).
//!
//! - Bounded üstel geri çekilme (exponential backoff)
//! - Hata türüne göre tekrar deneme kararı (auth/balance hatalarında denemez)
//! - Tavan gecikme süresi (`max_delay_ms`) ile sınırlandırma
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");
const error_taxonomy = @import("error_taxonomy.zig");
pub const ProviderErrorKind = error_taxonomy.ProviderErrorKind;

pub const RetryPolicy = struct {
    max_retries: u32 = 3,
    initial_delay_ms: u64 = 500,
    max_delay_ms: u64 = 30_000,
    backoff_multiplier: u32 = 2,

    pub const default: RetryPolicy = .{};

    /// Belirtilen deneme sayısı için bekleme süresini (ms) hesaplar.
    /// initial_delay_ms * (multiplier ^ attempt), max_delay_ms tavanı ile sınırlıdır.
    pub fn computeDelay(self: RetryPolicy, attempt: u32) u64 {
        if (attempt == 0) return self.initial_delay_ms;

        var delay = self.initial_delay_ms;
        var i: u32 = 0;
        while (i < attempt) : (i += 1) {
            const next_delay = std.math.mul(u64, delay, self.backoff_multiplier) catch self.max_delay_ms;
            if (next_delay >= self.max_delay_ms) {
                return self.max_delay_ms;
            }
            delay = next_delay;
        }

        return @min(delay, self.max_delay_ms);
    }

    /// Verilen hata türü ve deneme sayısına göre tekrar denenmeli mi karar verir.
    pub fn shouldRetry(self: RetryPolicy, err_kind: ProviderErrorKind, attempt: u32) bool {
        if (attempt >= self.max_retries) return false;
        return err_kind.isRetryable();
    }
};

test "retry gecikme hesaplama ve tavan siniri" {
    const policy = RetryPolicy{
        .max_retries = 5,
        .initial_delay_ms = 100,
        .max_delay_ms = 1000,
        .backoff_multiplier = 2,
    };

    try std.testing.expectEqual(@as(u64, 100), policy.computeDelay(0));
    try std.testing.expectEqual(@as(u64, 200), policy.computeDelay(1));
    try std.testing.expectEqual(@as(u64, 400), policy.computeDelay(2));
    try std.testing.expectEqual(@as(u64, 800), policy.computeDelay(3));
    try std.testing.expectEqual(@as(u64, 1000), policy.computeDelay(4)); // Tavan 1000
    try std.testing.expectEqual(@as(u64, 1000), policy.computeDelay(10));
}

test "retry karar kurallari" {
    const policy = RetryPolicy{ .max_retries = 3 };

    // Rate limited ve Server hataları limit altındayken retryable
    try std.testing.expect(policy.shouldRetry(.rate_limited, 0));
    try std.testing.expect(policy.shouldRetry(.server, 2));
    try std.testing.expect(!policy.shouldRetry(.server, 3)); // max_retries aşıldı

    // Auth ve Bakiye hataları asla tekrar denenmez
    try std.testing.expect(!policy.shouldRetry(.auth_failed, 0));
    try std.testing.expect(!policy.shouldRetry(.insufficient_balance, 0));
}
