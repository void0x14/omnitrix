//! Omnitrix Kol C (Permission ve Gerçeklik Kapısı) Kapsamlı Doğrulama Testleri.
//!
//! Tasarım: docs/superpowers/specs/2026-08-17-omnitrix-zig-runtime-change-ledger-design.md
//! Bölüm 9 Doğrulama Koşulları:
//! - Doğrulama 1: Agent gitignored dosyayı değiştirir; panelde/ledger'da görünür (C2).
//! - Doğrulama 2: Agent untracked dosya oluşturur; path, hash ve hunk görünür (C2).
//! - Doğrulama 3: User ve agent aynı dosyanın farklı hunk'larını değiştirir; actor ayrımı veya mixed_actor görünür (C4).
//! - Doğrulama 4: Dış süreç dosya değiştirir; agent değişikliği olarak yanlış etiketlenmez (C1, C4).
//! - Doğrulama 5: Agent shell komutuyla dosya değiştirir; operation sonrası ledger kaydı gelir (C3).
//! - Doğrulama 9: Büyük/binary/conflict dosyada sahte 0/0 gösterilmez (C6).
//! - Doğrulama 10: Eski hash ile revert denenir; işlem non-destructive şekilde reddedilir (C5).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const ledger_mod = @import("ledger.zig");
const mutation = @import("mutation.zig");
const hash_mod = @import("hash.zig");
const tool_harness = @import("tool_harness.zig");
const shell_snapshot = @import("shell_snapshot.zig");
const hunk_mod = @import("hunk.zig");
const attribution = @import("attribution.zig");
const revert = @import("revert.zig");
const watcher_debounce = @import("watcher_debounce.zig");
const permission = @import("../omnitrix-permission/permission.zig");

test "Dogrulama 1: Agent gitignored dosyayi degistirir; panelde gorunur" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = ledger_mod.Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    // 1. .gitignore dosyası oluştur
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = ".gitignore", .data = ".env\nlocal.log\n" });

    // 2. Agent, gitignored olan .env dosyasını yazar
    const ctx = permission.OperationContext{
        .operation_id = "op_env_write",
        .agent_id = "agent_juryrigg",
        .session_id = "sess_verify",
        .turn_id = 1,
    };

    const rec = try tool_harness.writeFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        ".env",
        "API_KEY=secret_value\nPORT=8080\n",
        ctx,
        &ledger,
        64 * 1024,
    );

    // Doğrulama: Gitignored dosyası ledger'da kayıtlı ve görünür
    try std.testing.expectEqualStrings(".env", rec.path);
    try std.testing.expect(rec.kind == .added);
    try std.testing.expect(rec.actor == .agent);
    try std.testing.expectEqualStrings("agent_juryrigg", rec.agent_id.?);
    try std.testing.expectEqualStrings("op_env_write", rec.operation_id.?);
}

test "Dogrulama 2: Agent untracked dosya olusturur; path, hash ve hunk gorunur" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = ledger_mod.Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const ctx = permission.OperationContext{
        .operation_id = "op_untracked_create",
        .agent_id = "agent_executor",
        .session_id = "sess_verify",
        .turn_id = 2,
    };

    const file_content = "pub fn calculate(x: i32) i32 {\n    return x * 2;\n}\n";
    const rec = try tool_harness.writeFile(
        tmp.dir,
        std.testing.io,
        std.testing.allocator,
        "src/calc.zig",
        file_content,
        ctx,
        &ledger,
        64 * 1024,
    );

    try std.testing.expectEqualStrings("src/calc.zig", rec.path);
    try std.testing.expect(rec.kind == .added);
    try std.testing.expect(rec.before_hash == null);
    try std.testing.expect(rec.after_hash != null);
    try std.testing.expectEqualSlices(u8, &hash_mod.hashBytes(file_content), &rec.after_hash.?);

    // Hunk hesaplaması: Boş tabandan yeni dosyaya tek bir ekleme hunk'ı
    var hunks = try hunk_mod.computeHunks(std.testing.allocator, "", file_content);
    defer hunks.deinit(std.testing.allocator);

    try std.testing.expectEqual(@as(usize, 1), hunks.items.len);
    try std.testing.expectEqual(@as(usize, 1), hunks.items[0].old_start);
    try std.testing.expect(hunks.items[0].new_count >= 1);
}

test "Dogrulama 3: User ve agent ayni dosyanin farkli hunk'larini degistirir; actor ayrimi ve mixed_actor" {
    const base_content =
        \\// Common Header
        \\// User managed section
        \\const USER_SETTING = 100;
        \\// Middle separator
        \\// Agent managed section
        \\const AGENT_SETTING = 200;
        \\// Common Footer
    ;

    // Durum A: Farklı hunk'lar -> Ayrı hunk sahipliği
    const user_modified =
        \\// Common Header
        \\// User managed section
        \\const USER_SETTING = 999;
        \\// Middle separator
        \\// Agent managed section
        \\const AGENT_SETTING = 200;
        \\// Common Footer
    ;

    const agent_modified =
        \\// Common Header
        \\// User managed section
        \\const USER_SETTING = 100;
        \\// Middle separator
        \\// Agent managed section
        \\const AGENT_SETTING = 555;
        \\// Common Footer
    ;

    var clean_attr = try attribution.resolveHunkAttribution(
        std.testing.allocator,
        "src/config.zig",
        base_content,
        user_modified,
        agent_modified,
        "agent_juryrigg",
        false,
    );
    defer clean_attr.deinit();

    try std.testing.expect(clean_attr.is_mixed);
    try std.testing.expect(clean_attr.kind == .modified);
    try std.testing.expectEqual(@as(usize, 2), clean_attr.hunks.items.len);
    try std.testing.expect(clean_attr.hunks.items[0].actor == .user);
    try std.testing.expect(clean_attr.hunks.items[1].actor == .agent);

    // Durum B: Çakışan hunk'lar -> mixed_actor
    const overlapping_user =
        \\// Common Header
        \\const SHARED_VALUE = "user_choice";
        \\// Middle separator
    ;
    const overlapping_agent =
        \\// Common Header
        \\const SHARED_VALUE = "agent_choice";
        \\// Middle separator
    ;

    var overlap_attr = try attribution.resolveHunkAttribution(
        std.testing.allocator,
        "src/overlap.zig",
        "// Common Header\nconst SHARED_VALUE = \"default\";\n// Middle separator",
        overlapping_user,
        overlapping_agent,
        "agent_juryrigg",
        false,
    );
    defer overlap_attr.deinit();

    try std.testing.expect(overlap_attr.kind == .mixed_actor);
    try std.testing.expect(overlap_attr.is_mixed);
    try std.testing.expect(overlap_attr.confidence <= 50);
}

test "Dogrulama 4: Dis surec dosya degistirir; agent degisikligi olarak yanlis etiketlenmez" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = ledger_mod.Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    // 1. Dış süreç (ör. background build veya başka bir program) dosya oluşturur
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "external_output.txt", .data = "build artifact" });

    // 2. Rescan ile tespit edilir
    const changed = try ledger.rescan(tmp.dir, std.testing.io, std.testing.allocator, 64 * 1024);
    try std.testing.expectEqual(@as(usize, 1), changed);

    // Doğrulama 4: Actor external'dır, agent_id null'dır, ajan sahiplenmemiştir
    const rec = ledger.get("external_output.txt").?;
    try std.testing.expectEqual(mutation.Actor.external, rec.actor);
    try std.testing.expect(rec.agent_id == null);
    try std.testing.expect(rec.operation_id == null);
}

test "Dogrulama 5: Agent shell komutuyla dosya degistirir; operation sonrasi ledger kaydi gelir" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = ledger_mod.Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "main.c", .data = "int main() { return 0; }" });

    // 1. Shell komutu öncesi bounded snapshot
    var pre_snap = try shell_snapshot.takeSnapshot(tmp.dir, std.testing.io, std.testing.allocator, .{}, 64 * 1024);
    defer pre_snap.deinit();

    // 2. Shell komutunun dosya oluşturduğunu ve var olanı değiştirdiğini simüle et
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "main.c", .data = "int main() { return 1; }" });
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "output.bin", .data = "binary content" });

    // 3. Shell komutu sonrası bounded snapshot
    var post_snap = try shell_snapshot.takeSnapshot(tmp.dir, std.testing.io, std.testing.allocator, .{}, 64 * 1024);
    defer post_snap.deinit();

    // 4. Snapshot diff
    var diff = try shell_snapshot.diffSnapshots(std.testing.allocator, &pre_snap, &post_snap);
    defer diff.deinit();

    const ctx = permission.OperationContext{
        .operation_id = "op_shell_gcc_run",
        .agent_id = "agent_builder",
        .session_id = "sess_verify",
        .turn_id = 5,
    };

    const count = try shell_snapshot.applyDiffToLedger(&diff, ctx, &ledger, 1234567);
    try std.testing.expectEqual(@as(usize, 2), count);

    // Doğrulama 5: Shell komutu sonrası ledger kayıtları ajana aittir
    const bin_rec = ledger.get("output.bin").?;
    try std.testing.expect(bin_rec.kind == .added);
    try std.testing.expect(bin_rec.actor == .agent);
    try std.testing.expectEqualStrings("op_shell_gcc_run", bin_rec.operation_id.?);

    const main_rec = ledger.get("main.c").?;
    try std.testing.expect(main_rec.kind == .modified);
    try std.testing.expect(main_rec.actor == .agent);
    try std.testing.expectEqualStrings("op_shell_gcc_run", main_rec.operation_id.?);
}

test "Dogrulama 9: Buyuk/binary/conflict dosyada sahte 0/0 gosterilmez" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = ledger_mod.Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    // 16 byte veri yazıp 4 byte limit vererek FileTooLarge -> binary olmasını sağla
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "blob.data", .data = "0123456789ABCDEF" });

    const changed = try ledger.rescan(tmp.dir, std.testing.io, std.testing.allocator, 4);
    try std.testing.expectEqual(@as(usize, 1), changed);

    const rec = ledger.get("blob.data").?;
    try std.testing.expect(rec.kind == .binary);
    // Sahte 0/0 gösterilmemelidir (additions ve deletions null olmalıdır)
    try std.testing.expect(rec.additions == null);
    try std.testing.expect(rec.deletions == null);
}

test "Dogrulama 10: Eski hash ile revert denenir; islem non-destructive sekilde reddedilir" {
    var tmp = std.testing.tmpDir(.{ .iterate = true });
    defer tmp.cleanup();
    var ledger = ledger_mod.Ledger.init(std.testing.allocator);
    defer ledger.deinit();

    const orig_text = "stable original content";
    const agent_text = "agent modified content";
    const user_external_text = "external actor tampered with file simultaneously";

    const h_orig = hash_mod.hashBytes(orig_text);
    const h_agent = hash_mod.hashBytes(agent_text);

    // Dosyaya harici değişiklik yazılır
    try tmp.dir.writeFile(std.testing.io, .{ .sub_path = "target.txt", .data = user_external_text });

    // Ledger ajan değişikliğini bekliyor (after_hash = h_agent)
    _ = try ledger.recordWithOptions(
        "target.txt",
        .modified,
        .agent,
        1000,
        h_agent,
        h_orig,
        .{},
    );

    // Eski hash ile revert denenir -> InterveningChange ile reddedilir
    try std.testing.expectError(
        error.InterveningChange,
        revert.safeRevertFile(
            tmp.dir,
            std.testing.io,
            std.testing.allocator,
            &ledger,
            "target.txt",
            orig_text,
            .{},
        ),
    );

    // Non-destructive: Diskteki dosya kesinlikle bozulmadı ve aynen korundu
    const disk_content = try tmp.dir.readFileAlloc(std.testing.io, "target.txt", std.testing.allocator, .limited(1024));
    defer std.testing.allocator.free(disk_content);
    try std.testing.expectEqualStrings(user_external_text, disk_content);
}
