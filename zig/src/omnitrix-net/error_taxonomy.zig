//! Provider Hata Taksonomisi ve Sınıflandırma (tasarım Bölüm 3.3, Doğrulama 5).
//!
//! 401/403 (auth), 429 (rate limit), bakiye yetersizliği (balance), transport,
//! timeout ve sunucu (5xx) hatalarını kesin olarak ayrıştırır.

const std = @import("std");

/// Provider çağrı hatası sınıfları.
pub const ProviderErrorKind = enum {
    auth_failed, // 401, 403 — kimlik doğrulama / yetki
    rate_limited, // 429 — hız sınırı
    insufficient_balance, // 402 veya provider bakiye yetersizliği
    transport, // ağ / soket / DNS / bağlantı kopması
    timeout, // deadline / istek zaman aşımı
    server, // 500..599 sunucu hatası
    unexpected, // sınıflandırılamayan diğer hatalar

    pub fn label(self: ProviderErrorKind) []const u8 {
        return @tagName(self);
    }

    /// Hataya karşı otomatik tekrar deneme (retry) yapılıp yapılmayacağı.
    pub fn isRetryable(self: ProviderErrorKind) bool {
        return switch (self) {
            .rate_limited, .transport, .timeout, .server => true,
            .auth_failed, .insufficient_balance, .unexpected => false,
        };
    }

    /// HTTP durum kodundan hata türünü çıkarır.
    pub fn fromHttpStatus(status: u16) ?ProviderErrorKind {
        return switch (status) {
            401, 403 => .auth_failed,
            402 => .insufficient_balance,
            429 => .rate_limited,
            500...599 => .server,
            else => null,
        };
    }

    /// HTTP durum kodu ve isteğe bağlı gövde (response body) metninden kesin sınıflandırma yapar.
    pub fn classify(status: ?u16, body: ?[]const u8) ProviderErrorKind {
        if (status) |code| {
            if (fromHttpStatus(code)) |kind| {
                // Özel durum: 400 veya 403 durumunda gövdede bakiye uyarısı olabilir
                if (body) |b| {
                    if (containsBalanceKeywords(b)) return .insufficient_balance;
                    if (containsRateLimitKeywords(b)) return .rate_limited;
                }
                return kind;
            }

            if (code == 400 or code == 422) {
                if (body) |b| {
                    if (containsBalanceKeywords(b)) return .insufficient_balance;
                    if (containsRateLimitKeywords(b)) return .rate_limited;
                    if (containsAuthKeywords(b)) return .auth_failed;
                }
            }
        }

        if (body) |b| {
            if (containsBalanceKeywords(b)) return .insufficient_balance;
            if (containsRateLimitKeywords(b)) return .rate_limited;
            if (containsAuthKeywords(b)) return .auth_failed;
        }

        return .unexpected;
    }

    fn containsBalanceKeywords(body: []const u8) bool {
        const keywords = [_][]const u8{
            "insufficient_quota",
            "credit_balance_too_low",
            "insufficient_funds",
            "quota_exceeded",
            "balance_depleted",
            "payment_required",
        };
        for (keywords) |kw| {
            if (std.mem.indexOf(u8, body, kw) != null) return true;
        }
        return false;
    }

    fn containsRateLimitKeywords(body: []const u8) bool {
        const keywords = [_][]const u8{
            "rate_limit_exceeded",
            "too_many_requests",
            "rate_limited",
            "request_limit_reached",
        };
        for (keywords) |kw| {
            if (std.mem.indexOf(u8, body, kw) != null) return true;
        }
        return false;
    }

    fn containsAuthKeywords(body: []const u8) bool {
        const keywords = [_][]const u8{
            "invalid_api_key",
            "authentication_failed",
            "unauthorized",
            "invalid_token",
            "permission_denied",
        };
        for (keywords) |kw| {
            if (std.mem.indexOf(u8, body, kw) != null) return true;
        }
        return false;
    }
};

test "http durum kodlari dogru siniflanir" {
    try std.testing.expectEqual(ProviderErrorKind.auth_failed, ProviderErrorKind.fromHttpStatus(401).?);
    try std.testing.expectEqual(ProviderErrorKind.auth_failed, ProviderErrorKind.fromHttpStatus(403).?);
    try std.testing.expectEqual(ProviderErrorKind.insufficient_balance, ProviderErrorKind.fromHttpStatus(402).?);
    try std.testing.expectEqual(ProviderErrorKind.rate_limited, ProviderErrorKind.fromHttpStatus(429).?);
    try std.testing.expectEqual(ProviderErrorKind.server, ProviderErrorKind.fromHttpStatus(503).?);
    try std.testing.expectEqual(@as(?ProviderErrorKind, null), ProviderErrorKind.fromHttpStatus(200));
}

test "body keyword analizi ile bakiye ve rate limit ayrilir" {
    const b1 = "{\"error\": {\"code\": \"insufficient_quota\", \"message\": \"You exceeded your current quota\"}}";
    try std.testing.expectEqual(ProviderErrorKind.insufficient_balance, ProviderErrorKind.classify(400, b1));

    const b2 = "{\"error\": \"rate_limit_exceeded\"}";
    try std.testing.expectEqual(ProviderErrorKind.rate_limited, ProviderErrorKind.classify(400, b2));

    const b3 = "{\"error\": \"invalid_api_key\"}";
    try std.testing.expectEqual(ProviderErrorKind.auth_failed, ProviderErrorKind.classify(null, b3));
}

test "retryable ozelligi" {
    try std.testing.expect(ProviderErrorKind.rate_limited.isRetryable());
    try std.testing.expect(ProviderErrorKind.server.isRetryable());
    try std.testing.expect(ProviderErrorKind.transport.isRetryable());
    try std.testing.expect(ProviderErrorKind.timeout.isRetryable());
    try std.testing.expect(!ProviderErrorKind.auth_failed.isRetryable());
    try std.testing.expect(!ProviderErrorKind.insufficient_balance.isRetryable());
}
