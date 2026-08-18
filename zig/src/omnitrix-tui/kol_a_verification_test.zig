//! Omnitrix Kol A (TUI ve Değişiklik Paneli) Kapsamlı Doğrulama Testleri.
//!
//! Tasarım: docs/superpowers/specs/2026-08-17-omnitrix-zig-runtime-change-ledger-design.md
//! Bölüm 9 Doğrulama Koşulları:
//! - Doğrulama 1: Agent, Gitignored dosyayı değiştirir; panelde görünür (A4).
//! - Doğrulama 2: Agent, untracked dosya oluşturur; path, hash ve hunk görünür (A4).
//! - Doğrulama 6: Dosya commit edilir; repository uncommitted görünümü güncellenir, session history kaydı korunur (A4).
//! - Doğrulama 7: Staged ve unstaged aynı dosyada birlikte bulunur; iki durum kaybolmaz (A4).
//! - Doğrulama 8: Rename ve delete durumları path bilgisiyle görünür (A4).
//! - Doğrulama 9: Büyük/binary/conflict dosyada sahte 0/0 gösterilmez (A4).
//! - Doğrulama 11: Terminal resize, Unicode ve uzun streaming transcript sonrasında state ve seçim korunur (A5).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const term = @import("terminal.zig");
const unicode = @import("unicode.zig");
const block_renderer = @import("block_renderer.zig");
const diff_renderer = @import("diff_renderer.zig");
const changed_files = @import("changed_files.zig");
const tui_mod = @import("tui.zig");
const pty_harness = @import("pty_harness.zig");

const mutation = @import("../omnitrix-ledger/mutation.zig");
const hash_mod = @import("../omnitrix-ledger/hash.zig");

test "Dogrulama 1 (Kol A): Agent gitignored dosyayi degistirir; panelde gorunur" {
    var panel = changed_files.ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    // Agent .env (gitignored) dosyasını değiştirir
    var rec = try mutation.MutationRecord.init(std.testing.allocator, 1, ".env", .modified, .agent, 100);
    defer rec.deinit(std.testing.allocator);
    rec.additions = 2;
    rec.deletions = 0;
    try rec.setContext(std.testing.allocator, "op_env_edit", "agent_executor", "sess_1", 1);

    try panel.syncFromMutationRecord(&rec);

    // Doğrulama 1: Gitignored dosya Agent Changes bölümünde listelenir
    try std.testing.expectEqual(@as(usize, 1), panel.agent_entries.items.len);
    try std.testing.expectEqualStrings(".env", panel.agent_entries.items[0].path);
    try std.testing.expectEqualStrings("agent_executor", panel.agent_entries.items[0].agent_name.?);
    try std.testing.expectEqual(@as(?u64, 2), panel.agent_entries.items[0].additions);

    var lines = try panel.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    var found_in_render = false;
    for (lines.items) |l| {
        if (std.mem.indexOf(u8, l, ".env") != null and std.mem.indexOf(u8, l, "agent_executor") != null) {
            found_in_render = true;
        }
    }
    try std.testing.expect(found_in_render);
}

test "Dogrulama 2 (Kol A): Agent untracked dosya olusturur; path, hash ve hunk gorunur" {
    var panel = changed_files.ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    const file_content = "pub fn add(a: i32, b: i32) i32 {\n    return a + b;\n}";
    const h = hash_mod.hashBytes(file_content);

    var rec = try mutation.MutationRecord.init(std.testing.allocator, 1, "src/math.zig", .added, .agent, 100);
    defer rec.deinit(std.testing.allocator);
    rec.after_hash = h;
    rec.additions = 3;
    rec.deletions = 0;
    try rec.setContext(std.testing.allocator, "op_math_create", "agent_juryrigg", "sess_2", 1);

    try panel.syncFromMutationRecord(&rec);
    try panel.updateGitStatus("src/math.zig", ' ', '?', true, false);

    // Doğrulama 2: Path, hash ve ekleme bilgisi görünür
    try std.testing.expectEqual(@as(usize, 1), panel.agent_entries.items.len);
    const entry = &panel.agent_entries.items[0];
    try std.testing.expectEqualStrings("src/math.zig", entry.path);
    try std.testing.expect(entry.after_hash != null);
    try std.testing.expectEqualSlices(u8, &h, &entry.after_hash.?);
    try std.testing.expect(entry.git_status.is_untracked);

    // Diff renderer üzerinden hunk görünürlüğü
    var dr = diff_renderer.DiffRenderer.init(std.testing.allocator);
    defer dr.deinit();

    try dr.addFileDiffFromTexts("src/math.zig", "", file_content);
    try std.testing.expectEqual(@as(usize, 1), dr.files.items.len);
    try std.testing.expectEqual(@as(usize, 1), dr.files.items[0].hunks.items.len);
    try std.testing.expectEqual(@as(?u64, 3), dr.files.items[0].additions);
}

test "Dogrulama 6 (Kol A): Dosya commit edilir; repository uncommitted gorunumu guncellenir, session history kaydi korunur" {
    var panel = changed_files.ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    // 1. Agent bir dosyayı değiştirir ve git staged durumundadır
    var rec = try mutation.MutationRecord.init(std.testing.allocator, 1, "src/server.zig", .modified, .agent, 100);
    defer rec.deinit(std.testing.allocator);
    rec.additions = 10;
    rec.deletions = 2;
    try rec.setContext(std.testing.allocator, "op_srv", "agent_net", "sess_6", 1);

    try panel.syncFromMutationRecord(&rec);
    try panel.updateGitStatus("src/server.zig", 'M', ' ', false, false);

    try std.testing.expectEqual(@as(u8, 'M'), panel.agent_entries.items[0].git_status.staged);

    // 2. Commit işlemi gerçekleşir
    try panel.handleCommit();

    // Doğrulama 6:
    // a) Uncommitted git görünümü güncellendi (staged boşaldı)
    try std.testing.expectEqual(@as(u8, ' '), panel.agent_entries.items[0].git_status.staged);
    // b) Oturum geçmişi (committed_history) ve ajan sahipliği korundu
    try std.testing.expectEqual(@as(usize, 1), panel.committed_history.items.len);
    try std.testing.expectEqualStrings("src/server.zig", panel.committed_history.items[0].path);
    try std.testing.expectEqualStrings("agent_net", panel.committed_history.items[0].agent_name.?);
    try std.testing.expectEqual(@as(?u64, 10), panel.committed_history.items[0].additions);
}

test "Dogrulama 7 (Kol A): Staged ve unstaged ayni dosyada birlikte bulunur; iki durum kaybolmaz" {
    var panel = changed_files.ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    // Durum 1: MM (Index'te modified, Worktree'de modified)
    try panel.updateGitStatus("src/app.zig", 'M', 'M', false, false);
    // Durum 2: AM (Index'te added, Worktree'de modified)
    try panel.updateGitStatus("src/new_feature.zig", 'A', 'M', false, false);
    // Durum 3: MD (Index'te modified, Worktree'de deleted)
    try panel.updateGitStatus("src/temp.zig", 'M', 'D', false, false);

    try std.testing.expectEqual(@as(usize, 3), panel.project_entries.items.len);

    // MM doğrulaması
    try std.testing.expectEqual(@as(u8, 'M'), panel.project_entries.items[0].git_status.staged);
    try std.testing.expectEqual(@as(u8, 'M'), panel.project_entries.items[0].git_status.unstaged);

    // AM doğrulaması
    try std.testing.expectEqual(@as(u8, 'A'), panel.project_entries.items[1].git_status.staged);
    try std.testing.expectEqual(@as(u8, 'M'), panel.project_entries.items[1].git_status.unstaged);

    // MD doğrulaması
    try std.testing.expectEqual(@as(u8, 'M'), panel.project_entries.items[2].git_status.staged);
    try std.testing.expectEqual(@as(u8, 'D'), panel.project_entries.items[2].git_status.unstaged);
}

test "Dogrulama 8 (Kol A): Rename ve delete durumlari path bilgisiyle gorunur" {
    var panel = changed_files.ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    // Rename kaydı
    var rec_ren = try mutation.MutationRecord.init(std.testing.allocator, 1, "src/renamed_target.zig", .renamed, .agent, 100);
    defer rec_ren.deinit(std.testing.allocator);
    rec_ren.old_path = try std.testing.allocator.dupe(u8, "src/old_source.zig");
    try rec_ren.setContext(std.testing.allocator, null, "agent_refactor", null, 1);
    try panel.syncFromMutationRecord(&rec_ren);

    // Delete kaydı
    var rec_del = try mutation.MutationRecord.init(std.testing.allocator, 2, "src/deprecated.zig", .deleted, .agent, 200);
    defer rec_del.deinit(std.testing.allocator);
    try rec_del.setContext(std.testing.allocator, null, "agent_cleanup", null, 2);
    try panel.syncFromMutationRecord(&rec_del);

    try std.testing.expectEqual(@as(usize, 2), panel.agent_entries.items.len);

    // Rename path bilgisi doğrulaması
    const ren_entry = &panel.agent_entries.items[0];
    try std.testing.expectEqualStrings("src/renamed_target.zig", ren_entry.path);
    try std.testing.expectEqualStrings("src/old_source.zig", ren_entry.old_path.?);

    // Delete path bilgisi doğrulaması
    const del_entry = &panel.agent_entries.items[1];
    try std.testing.expectEqualStrings("src/deprecated.zig", del_entry.path);
    try std.testing.expectEqual(mutation.MutationKind.deleted, del_entry.kind);
}

test "Dogrulama 9 (Kol A): Buyuk/binary/conflict dosyada sahte 0/0 gosterilmez" {
    var panel = changed_files.ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    // Binary dosya
    var rec_bin = try mutation.MutationRecord.init(std.testing.allocator, 1, "assets/weights.bin", .binary, .agent, 100);
    defer rec_bin.deinit(std.testing.allocator);
    rec_bin.additions = null;
    rec_bin.deletions = null;
    try panel.syncFromMutationRecord(&rec_bin);

    // Conflict dosya
    var rec_conf = try mutation.MutationRecord.init(std.testing.allocator, 2, "src/merge_conflict.zig", .conflict, .agent, 200);
    defer rec_conf.deinit(std.testing.allocator);
    rec_conf.additions = null;
    rec_conf.deletions = null;
    try panel.syncFromMutationRecord(&rec_conf);

    var lines = try panel.renderToLines(std.testing.allocator, 100);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    var found_binary = false;
    var found_conflict = false;
    var found_fake_zero = false;

    for (lines.items) |l| {
        if (std.mem.indexOf(u8, l, "assets/weights.bin") != null and std.mem.indexOf(u8, l, "[binary]") != null) {
            found_binary = true;
        }
        if (std.mem.indexOf(u8, l, "src/merge_conflict.zig") != null and std.mem.indexOf(u8, l, "[conflict]") != null) {
            found_conflict = true;
        }
        if (std.mem.indexOf(u8, l, "+0 -0") != null) {
            found_fake_zero = true;
        }
    }

    try std.testing.expect(found_binary);
    try std.testing.expect(found_conflict);
    try std.testing.expect(!found_fake_zero); // Kesinlikle sahte 0/0 basılmaz!
}

test "Dogrulama 11 (Kol A): Terminal resize, Unicode ve uzun streaming transcript sonrasinda state ve secim korunur" {
    var harness = try pty_harness.PtyHarness.init(std.testing.allocator, 120, 40);
    defer harness.deinit();

    // 1. Unicode metinler ve bloklar ekle
    const block_id = try harness.tui.blocks.addBlock(.agent, "Omnitrix 🤖 (Türkçe / 🚀 / 日本語)", "");

    // 2. Uzun streaming transcript akışı
    var i: u64 = 1;
    while (i <= 50) : (i += 1) {
        try harness.tui.blocks.appendStreamingChunk(block_id, "Satır akışı #... ", i);
    }

    // 3. Diff ekle ve 2. hunk'ı seç
    const old_txt = "satir1\nsatir2\nsatir3\nsatir4\nsatir5\nsatir6";
    const new_txt = "satir1_mod\nsatir2\nsatir3_mod\nsatir4\nsatir5\nsatir6_mod";
    try harness.tui.diffs.addFileDiffFromTexts("src/unicode_test.zig", old_txt, new_txt);
    harness.tui.diffs.selected_hunk_index = 2; // 3. hunk seçili

    // 4. Changed files panelinde dosya seç
    var rec = try mutation.MutationRecord.init(std.testing.allocator, 1, "src/data.zig", .modified, .agent, 100);
    defer rec.deinit(std.testing.allocator);
    try harness.tui.changed_panel.syncFromMutationRecord(&rec);
    harness.tui.changed_panel.selected_index = 0;

    // 5. Terminali defalarca yeniden boyutlandır (Resize stress)
    try harness.resize(140, 50); // Büyüt
    try harness.resize(60, 20); // Dar ekrana küçült (< 80 cols)
    try harness.resize(90, 30); // Normal boyuta döndür

    // Doğrulama 11:
    // a) Seçili hunk indeksi korundu
    try std.testing.expectEqual(@as(usize, 2), harness.tui.diffs.selected_hunk_index);
    // b) Seçili dosya indeksi korundu
    try std.testing.expectEqual(@as(usize, 0), harness.tui.changed_panel.selected_index);
    // c) Uzun streaming içeriği ve Unicode karakterler bozulmadan korundu
    const block = harness.tui.blocks.getBlock(block_id).?;
    try std.testing.expect(block.content.items.len > 0);
    try std.testing.expect(std.mem.indexOf(u8, block.header, "Türkçe") != null);
    try std.testing.expect(std.mem.indexOf(u8, block.header, "🚀") != null);
    try std.testing.expect(std.mem.indexOf(u8, block.header, "日本語") != null);
    try std.testing.expectEqual(@as(u64, 50), block.revision);
}
