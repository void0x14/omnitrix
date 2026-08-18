//! omnitrix-permission: Doom-loop dedektörü ve circuit-breaker (tasarım Bölüm 3.2, 7 Kol C - C1).
//!
//! Ajanın aynı tool'u aynı argümanlarla tekrar tekrar çağırmasını (identical call loop)
//! veya aynı hatayı sürekli almasını (identical error loop) tespit eder.
//! Eşik aşıldığında circuit-breaker devreye girer ve otomatik döngüyü kırar.
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");

pub const DoomLoopError = error{
    DoomLoopDetected,
    CircuitBreakerTripped,
} || std.mem.Allocator.Error;

pub const DoomLoopConfig = struct {
    /// Aynı tool + aynı argümanla yapılabilecek maksimum tekrar sayısı.
    max_identical_calls: u32 = 3,
    /// Üst üste gelebilecek aynı hata sayısı.
    max_consecutive_errors: u32 = 3,
    /// Geçmiş arabellek pencere boyutu (bounded memory, Bölüm 8).
    window_size: usize = 32,
};

pub const CallSignature = struct {
    tool_name_hash: u64,
    args_hash: u64,
};

pub const ErrorSignature = struct {
    tool_name_hash: u64,
    err_msg_hash: u64,
};

pub const DoomLoopDetector = struct {
    config: DoomLoopConfig,
    allocator: std.mem.Allocator,

    // Çağrı geçmişi (bounded)
    recent_calls: std.ArrayList(CallSignature),
    identical_call_count: u32 = 0,
    last_call: ?CallSignature = null,

    // Hata geçmişi
    consecutive_error_count: u32 = 0,
    last_error: ?ErrorSignature = null,

    is_tripped: bool = false,
    trip_reason: ?[]const u8 = null,

    pub fn init(allocator: std.mem.Allocator, config: DoomLoopConfig) DoomLoopDetector {
        return .{
            .config = config,
            .allocator = allocator,
            .recent_calls = std.ArrayList(CallSignature).empty,
            .identical_call_count = 0,
            .last_call = null,
            .consecutive_error_count = 0,
            .last_error = null,
            .is_tripped = false,
            .trip_reason = null,
        };
    }

    pub fn deinit(self: *DoomLoopDetector) void {
        if (self.trip_reason) |r| self.allocator.free(r);
        self.recent_calls.deinit(self.allocator);
        self.* = undefined;
    }

    fn hashString(str: []const u8) u64 {
        return std.hash.Wyhash.hash(0, str);
    }

    /// Bir tool çağrısını kaydeder ve doom loop kontrolü yapar.
    pub fn recordCall(self: *DoomLoopDetector, tool_name: []const u8, args_json: []const u8) DoomLoopError!void {
        if (self.is_tripped) return error.CircuitBreakerTripped;

        const current_sig = CallSignature{
            .tool_name_hash = hashString(tool_name),
            .args_hash = hashString(args_json),
        };

        // 1. Ardışık aynı çağrı kontrolü
        if (self.last_call) |last| {
            if (last.tool_name_hash == current_sig.tool_name_hash and last.args_hash == current_sig.args_hash) {
                self.identical_call_count += 1;
            } else {
                self.identical_call_count = 1;
            }
        } else {
            self.identical_call_count = 1;
        }
        self.last_call = current_sig;

        // 2. Sliding window frekans kontrolü
        var window_matches: u32 = 1;
        for (self.recent_calls.items) |sig| {
            if (sig.tool_name_hash == current_sig.tool_name_hash and sig.args_hash == current_sig.args_hash) {
                window_matches += 1;
            }
        }

        if (self.identical_call_count >= self.config.max_identical_calls or window_matches >= self.config.max_identical_calls) {
            try self.trip("Identical tool call threshold exceeded");
            return error.DoomLoopDetected;
        }

        // Bounded geçmiş listesi
        if (self.recent_calls.items.len >= self.config.window_size) {
            _ = self.recent_calls.orderedRemove(0);
        }
        try self.recent_calls.append(self.allocator, current_sig);
    }

    /// Başarılı bir tool sonucunu kaydeder (hata sayacını sıfırlar).
    pub fn recordSuccess(self: *DoomLoopDetector) void {
        self.consecutive_error_count = 0;
        self.last_error = null;
    }

    /// Hata sonucunu kaydeder ve hata döngüsü kontrolü yapar.
    pub fn recordError(self: *DoomLoopDetector, tool_name: []const u8, err_msg: []const u8) DoomLoopError!void {
        if (self.is_tripped) return error.CircuitBreakerTripped;

        const err_sig = ErrorSignature{
            .tool_name_hash = hashString(tool_name),
            .err_msg_hash = hashString(err_msg),
        };

        if (self.last_error) |last| {
            if (last.tool_name_hash == err_sig.tool_name_hash and last.err_msg_hash == err_sig.err_msg_hash) {
                self.consecutive_error_count += 1;
                if (self.consecutive_error_count >= self.config.max_consecutive_errors) {
                    try self.trip("Identical consecutive error threshold exceeded");
                    return error.DoomLoopDetected;
                }
            } else {
                self.consecutive_error_count = 1;
            }
        } else {
            self.consecutive_error_count = 1;
        }

        self.last_error = err_sig;
    }

    /// Circuit breaker'ı tetikler.
    pub fn trip(self: *DoomLoopDetector, reason: []const u8) !void {
        self.is_tripped = true;
        if (self.trip_reason) |r| self.allocator.free(r);
        self.trip_reason = try self.allocator.dupe(u8, reason);
    }

    /// Kullanıcı müdahalesi veya yeni tur sonrasında durumu sıfırlar.
    pub fn reset(self: *DoomLoopDetector) void {
        self.identical_call_count = 0;
        self.last_call = null;
        self.consecutive_error_count = 0;
        self.last_error = null;
        self.recent_calls.clearRetainingCapacity();
        self.is_tripped = false;
        if (self.trip_reason) |r| {
            self.allocator.free(r);
            self.trip_reason = null;
        }
    }

    pub fn isTripped(self: *const DoomLoopDetector) bool {
        return self.is_tripped;
    }
};

test "doom loop: ayni cagri esigi asinca trip eder" {
    var detector = DoomLoopDetector.init(std.testing.allocator, .{
        .max_identical_calls = 3,
        .max_consecutive_errors = 3,
    });
    defer detector.deinit();

    try detector.recordCall("read_file", "{\"path\":\"a.txt\"}");
    try detector.recordCall("read_file", "{\"path\":\"a.txt\"}");
    // 3. aynı çağrıda DoomLoopDetected döner
    try std.testing.expectError(error.DoomLoopDetected, detector.recordCall("read_file", "{\"path\":\"a.txt\"}"));
    try std.testing.expect(detector.isTripped());

    // Sonraki çağrılarda CircuitBreakerTripped
    try std.testing.expectError(error.CircuitBreakerTripped, detector.recordCall("read_file", "{\"path\":\"a.txt\"}"));

    // Reset sonrası normal çalışır
    detector.reset();
    try std.testing.expect(!detector.isTripped());
    try detector.recordCall("read_file", "{\"path\":\"a.txt\"}");
}

test "doom loop: farkli argumanlar doom loop sayilmaz" {
    var detector = DoomLoopDetector.init(std.testing.allocator, .{
        .max_identical_calls = 3,
        .max_consecutive_errors = 3,
    });
    defer detector.deinit();

    try detector.recordCall("read_file", "{\"path\":\"a.txt\"}");
    try detector.recordCall("read_file", "{\"path\":\"b.txt\"}");
    try detector.recordCall("read_file", "{\"path\":\"c.txt\"}");
    try std.testing.expect(!detector.isTripped());
}

test "doom loop: ayni hata ust uste gelirse trip eder" {
    var detector = DoomLoopDetector.init(std.testing.allocator, .{
        .max_identical_calls = 3,
        .max_consecutive_errors = 3,
    });
    defer detector.deinit();

    try detector.recordError("edit_file", "FileNotFound");
    try detector.recordError("edit_file", "FileNotFound");
    try std.testing.expectError(error.DoomLoopDetected, detector.recordError("edit_file", "FileNotFound"));
    try std.testing.expect(detector.isTripped());
}
