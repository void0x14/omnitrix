//! Omnitrix Zig runtime — minimal giriş noktası.
//!
//! Bu aşamada TUI yok (Kol A ayrı task); uygulama başlatıldığında çekirdek
//! sözleşmelerin derlendiğini doğrular ve sürüm bilgisi basar. Runtime/TUI
//! katmanları planın Kol A/B task'larında geliştirilir.

const std = @import("std");
const omnitrix = @import("omnitrix");

pub const version = "0.0.0";

pub fn main() void {
    std.debug.print("omnitrix (zig runtime) {s}\n", .{version});
    _ = omnitrix; // çekirdek modüller derleme bağımlılığı
}
