//! Omnitrix — Tek-Süreç Zig Runtime & TUI Ana Giriş Noktası
//!
//! Şartname: docs/superpowers/specs/2026-08-17-omnitrix-zig-runtime-change-ledger-design.md
//! Plan: docs/superpowers/plans/2026-08-17-omnitrix-zig-runtime-change-ledger.md
//!
//! Komutlar:
//!   omnitrix [tui]       - Etkileşimli TUI arayüzünü başlatır (Varsayılan)
//!   omnitrix scan        - Proje dizinini yetkili şekilde tarar ve durum özeti çıkarır
//!   omnitrix status      - Ledger ve Git durumunu listeler
//!   omnitrix soak        - 24/7 Soak ve performans kapılarını çalıştırır
//!   omnitrix version     - Sürüm ve derleme bilgilerini basar
//!   omnitrix help        - Kullanım yardımını gösterir

const std = @import("std");
const omnitrix = @import("omnitrix");

pub const version = "0.1.0-alpha";

pub fn main(init: std.process.Init) !void {
    const allocator = init.gpa;
    const io = init.io;

    var it = try std.process.Args.Iterator.initAllocator(init.minimal.args, allocator);
    defer it.deinit();

    _ = it.skip(); // program adı

    const command = it.next() orelse "tui";

    if (std.mem.eql(u8, command, "version") or std.mem.eql(u8, command, "--version") or std.mem.eql(u8, command, "-v")) {
        printVersion();
        return;
    }

    if (std.mem.eql(u8, command, "help") or std.mem.eql(u8, command, "--help") or std.mem.eql(u8, command, "-h")) {
        printHelp();
        return;
    }

    if (std.mem.eql(u8, command, "scan")) {
        try runScan(allocator, io);
        return;
    }

    if (std.mem.eql(u8, command, "status")) {
        try runStatus(allocator, io);
        return;
    }

    if (std.mem.eql(u8, command, "soak")) {
        try runSoak(allocator);
        return;
    }

    if (std.mem.eql(u8, command, "tui")) {
        try runTui(allocator, io);
        return;
    }

    std.debug.print("Bilinmeyen komut: '{s}'\n\n", .{command});
    printHelp();
    std.process.exit(1);
}

fn printVersion() void {
    std.debug.print(
        \\Omnitrix Single-Process Zig Runtime (v{s})
        \\Mimariler: omnitrix-io (epoll), omnitrix-task, omnitrix-net, FileMutationLedger, Omnitrix-TUI
        \\
    , .{version});
}

fn printHelp() void {
    std.debug.print(
        \\Omnitrix — Autonomous Agentic Coding Runtime & File Mutation Ledger
        \\
        \\KULLANIM:
        \\  omnitrix [KOMUT]
        \\
        \\KOMUTLAR:
        \\  tui        Etkileşimli TUI arayüzünü başlatır (Varsayılan)
        \\  scan       Proje kökünü tarar; VCS/.cache hariç, ignored dosyalar dahil listeler
        \\  status     FileMutationLedger ve Git değişikliklerini gösterir
        \\  soak       24/7 Soak, bellek sızıntısı ve orphan task kapılarını test eder
        \\  version    Sürüm bilgisini basar
        \\  help       Bu yardım mesajını gösterir
        \\
        \\TUI KONTROLLERİ:
        \\  Tab        Paneller arası geçiş yap (Conversation / Changed Files / Diff)
        \\  1, 2, 3    Doğrudan ilgili panele odaklan
        \\  j / k      Listelerde veya Diff hunk'larında yukarı/aşağı gezin
        \\  Space      Agent vs Project değişiklik listeleri arasında geçiş yap
        \\  t          Aktif tool bloğunu katla/genişlet
        \\  q / Ctrl+C Çıkış yap
        \\
    , .{});
}

fn runScan(allocator: std.mem.Allocator, io: std.Io) !void {
    std.debug.print("🔍 Proje kökü taranıyor...\n", .{});
    const cur_dir = std.Io.Dir.cwd();
    var project_dir = cur_dir.openDir(io, ".", .{ .iterate = true }) catch cur_dir;
    defer if (project_dir.handle != cur_dir.handle) project_dir.close(io);

    var scan_res = try omnitrix.scan.scanProject(project_dir, io, allocator, .{});
    defer scan_res.deinit();

    std.debug.print("✅ Tarama tamamlandı:\n", .{});
    std.debug.print("  - Bulunan dosya sayısı : {d}\n", .{scan_res.files.items.len});
    std.debug.print("  - Atlanan özel klasör : {d} (VCS metadata, zig-cache vb.)\n", .{scan_res.skipped_dirs});

    const max_display = if (scan_res.files.items.len > 10) 10 else scan_res.files.items.len;
    std.debug.print("\nÖrnek taranan dosyalar (İlk {d}):\n", .{max_display});
    for (scan_res.files.items[0..max_display]) |file| {
        std.debug.print("    📄 {s}\n", .{file});
    }
    if (scan_res.files.items.len > max_display) {
        std.debug.print("    ... ve {d} dosya daha\n", .{scan_res.files.items.len - max_display});
    }
}

fn runStatus(allocator: std.mem.Allocator, io: std.Io) !void {
    std.debug.print("📊 FileMutationLedger Durumu İnceleniyor...\n", .{});
    const cur_dir = std.Io.Dir.cwd();
    var project_dir = cur_dir.openDir(io, ".", .{ .iterate = true }) catch cur_dir;
    defer if (project_dir.handle != cur_dir.handle) project_dir.close(io);

    var ledger = omnitrix.ledger.Ledger.init(allocator);
    defer ledger.deinit();

    const changed_count = try ledger.rescan(project_dir, io, allocator, 64 * 1024);
    std.debug.print("✅ Ledger güncellendi. Kayıtlı girdi sayısı: {d}, Algılanan mutasyon: {d}\n", .{
        ledger.entryCount(),
        changed_count,
    });
}

fn runSoak(allocator: std.mem.Allocator) !void {
    std.debug.print("🚀 24/7 Soak ve Performans Kapıları Çalıştırılıyor...\n\n", .{});
    const report = try omnitrix.soak.runFullSoakGate(allocator);

    std.debug.print("============================================================\n", .{});
    std.debug.print("SOAK KAPILARI SONUÇ RAPORU\n", .{});
    std.debug.print("============================================================\n", .{});
    std.debug.print("  - Bounded Queue Stress Kapısı : {s}\n", .{if (report.queue_gate_passed) "✅ GEÇTİ" else "❌ KALDI"});
    std.debug.print("  - Memory Growth & Leak Kapısı : {s} (Net Sızıntı: {d} B)\n", .{
        if (report.memory_gate_passed) "✅ GEÇTİ" else "❌ KALDI",
        report.final_leaked_bytes,
    });
    std.debug.print("  - Orphan Task Watchdog Kapısı : {s} (Temizlenen Orphan: {d})\n", .{
        if (report.orphan_gate_passed) "✅ GEÇTİ" else "❌ KALDI",
        report.orphans_reaped,
    });
    std.debug.print("  - Deterministic Shutdown      : {s} (Süre: {d:.2} ms)\n", .{
        if (report.shutdown_gate_passed) "✅ GEÇTİ" else "❌ KALDI",
        report.shutdown_duration_ms,
    });
    std.debug.print("  - Peak Bellek Ayrımı         : {d} B\n", .{report.peak_memory_bytes});
    std.debug.print("  - Müdahale Gerektiren Hata    : {d}\n", .{report.unhandled_errors});
    std.debug.print("------------------------------------------------------------\n", .{});
    std.debug.print("GENEL DEĞERLENDİRME: {s}\n", .{if (report.all_passed) "🏆 %100 BAŞARILI (TÜM KAPILAR GEÇİLDİ)" else "⚠️ BAZI KAPILAR BAŞARISIZ"});
    std.debug.print("============================================================\n", .{});
}

fn runTui(allocator: std.mem.Allocator, io: std.Io) !void {
    const is_tty = (std.Io.File.stdout().isTty(io) catch false) and (std.Io.File.stdin().isTty(io) catch false);

    if (!is_tty) {
        std.debug.print("⚠️ Terminal TTY modunda değil. Headless simülasyon karesi oluşturuluyor...\n", .{});
        var mock = try omnitrix.mock_terminal.MockTerminal.init(allocator, 100, 30);
        defer mock.deinit();

        var app = try omnitrix.tui.Tui.init(allocator, mock.backend(), 100);
        defer app.deinit();

        _ = try app.blocks.addBlock(.system, "System", "Omnitrix single-process Zig runtime başlatıldı.");
        _ = try app.blocks.addBlock(.user, "User", "omnitrix --tui headless test");
        _ = try app.blocks.addBlock(.agent, "Omnitrix", "Tüm sistemler aktif: IO EventLoop, TaskScheduler, FileMutationLedger ve TUI renderer devrede.");

        try app.renderFrame();
        std.debug.print("✅ Headless render başarıyla tamamlandı.\n", .{});
        return;
    }

    var posix_term = omnitrix.terminal.PosixTerminal.init(allocator);
    defer posix_term.deinit();

    var guard = try omnitrix.terminal.TerminalGuard.init(posix_term.backend());
    defer guard.deinit();

    var app = try omnitrix.tui.Tui.init(allocator, posix_term.backend(), 200);
    defer app.deinit();

    _ = try app.blocks.addBlock(.system, "System", "Omnitrix Autonomous Runtime v0.1.0 (single-process) initialized.");
    _ = try app.blocks.addBlock(.user, "Operator", "Scan codebase and prepare multi-agent execution pipeline.");
    _ = try app.blocks.addToolBlock("scan_project", "path: .", "Scanned 51 source files across 8 packages. 0 errors.");
    _ = try app.blocks.addBlock(.agent, "Omnitrix", "Codebase mapped. EventLoop, FileMutationLedger and TaskScheduler ready.\n• Press Tab to switch panels (Files / Diff)\n• Press Ctrl+V or type /voice for Grok Voice Mode\n• Type / for command palette");

    // İlk taramayı changed panel'e aktar
    const cur_dir = std.Io.Dir.cwd();
    var project_dir = cur_dir.openDir(io, ".", .{ .iterate = true }) catch cur_dir;
    defer if (project_dir.handle != cur_dir.handle) project_dir.close(io);

    var scan_res = try omnitrix.scan.scanProject(project_dir, io, allocator, .{ .max_entries = 100 });
    defer scan_res.deinit();

    for (scan_res.files.items) |f| {
        try app.changed_panel.addProjectChange(f, .external);
    }

    // Örnek Agent değişikliği
    try app.changed_panel.addAgentChange("src/root.zig", .modified, 45, 2, "executor");
    try app.changed_panel.addAgentChange("src/omnitrix-io/epoll.zig", .added, 120, 0, "io_agent");

    // Canlı etkileşim döngüsü
    var input_buf: [32]u8 = undefined;
    while (app.is_running) {
        const sz = try posix_term.getSize();
        if (sz.cols != app.size.cols or sz.rows != app.size.rows) {
            app.handleResize(sz.cols, sz.rows);
        }

        try app.renderFrame();

        const n = try posix_term.readInput(&input_buf);
        if (n > 0) {
            try app.handleKey(input_buf[0..n]);
        }

        const req = std.os.linux.timespec{ .sec = 0, .nsec = 30 * std.time.ns_per_ms };
        _ = std.os.linux.nanosleep(&req, null);
    }
}
