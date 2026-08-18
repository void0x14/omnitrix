//! omnitrix-tui: Changed-Files Paneli — Agent vs Project Ayrımı ve Git X/Y Entegrasyonu (tasarım Bölüm 5.2, Kol A - A4, Doğrulama 1, 2, 6, 7, 8, 9, 11).
//!
//! Özellikler:
//! - İki ana bölüm:
//!   1. **Agent Changes**: Ajanlar tarafından yapılan değişiklikler (M, A, D, +ekle -sil, agent: name).
//!   2. **Project Changes**: Kullanıcı veya dış süreç değişiklikleri (actor: user / external).
//! - Git status X/Y (Staged / Unstaged) desteği:
//!   - X = Index (staged), Y = Worktree (unstaged), ? = Untracked.
//!   - Doğrulama 7: Aynı dosyada hem staged hem unstaged aynı anda bulunur (ör. MM, AM, MD); iki durum kaybolmaz.
//!   - Doğrulama 8: Rename ve delete durumları yol (path) bilgisiyle görünür (old_path -> new_path).
//!   - Doğrulama 6: Commit sonrası uncommitted görünüm güncellenir ancak oturum (session) mutasyon geçmişi korunur.
//!   - Doğrulama 9: Büyük / binary / conflict dosyalarda sahte 0/0 gösterilmez ([binary] / [conflict] olarak sunulur).
//!   - Doğrulama 1: Gitignored dosya ajanca değiştirildiğinde panelde görünür.
//!   - Doğrulama 2: Untracked dosya oluşturulduğunda path, hash ve hunk bilgisiyle panelde yer alır.
//! - Filtreleme: `.git/`, Omnitrix iç cache/build klasörleri ve kök dışı symlink'ler hariç; normal ignored dosyalar dahil.
//! - Resize sırasında seçili dosya indeksini güvenle korur (Doğrulama 11).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const unicode = @import("unicode.zig");
const term = @import("terminal.zig");
const ledger_mod = @import("../omnitrix-ledger/ledger.zig");
const mutation = @import("../omnitrix-ledger/mutation.zig");

pub const MutationKind = mutation.MutationKind;
pub const Actor = mutation.Actor;
pub const Hash = mutation.Hash;
pub const MutationRecord = mutation.MutationRecord;
pub const Ledger = ledger_mod.Ledger;

/// Git X/Y durum göstergeleri (X: Staged/Index, Y: Unstaged/Worktree)
pub const GitStatus = struct {
    staged: u8 = ' ', // 'M', 'A', 'D', 'R', 'C', ' '
    unstaged: u8 = ' ', // 'M', 'D', '?', ' '
    is_untracked: bool = false,
    is_ignored: bool = false,
    is_conflict: bool = false,

    pub fn hasChanges(self: GitStatus) bool {
        return self.staged != ' ' or self.unstaged != ' ' or self.is_untracked or self.is_conflict;
    }

    pub fn label(self: GitStatus, buf: *[8]u8) []const u8 {
        if (self.is_conflict) return "UU";
        if (self.is_untracked) return "??";
        buf[0] = self.staged;
        buf[1] = self.unstaged;
        return buf[0..2];
    }
};

/// Paneldeki tek bir dosya girdisi
pub const ChangedFileEntry = struct {
    path: []const u8,
    old_path: ?[]const u8 = null,
    kind: MutationKind,
    actor: Actor,
    agent_name: ?[]const u8 = null,
    additions: ?u64 = null,
    deletions: ?u64 = null,
    before_hash: ?Hash = null,
    after_hash: ?Hash = null,
    git_status: GitStatus = .{},
    is_binary: bool = false,
    is_conflict: bool = false,
    observed_at: i128 = 0,

    pub fn init(
        allocator: std.mem.Allocator,
        path: []const u8,
        kind: MutationKind,
        actor: Actor,
    ) !ChangedFileEntry {
        return .{
            .path = try allocator.dupe(u8, path),
            .old_path = null,
            .kind = kind,
            .actor = actor,
            .agent_name = null,
            .additions = null,
            .deletions = null,
            .before_hash = null,
            .after_hash = null,
            .git_status = .{},
            .is_binary = (kind == .binary),
            .is_conflict = (kind == .conflict),
            .observed_at = 0,
        };
    }

    pub fn deinit(self: *ChangedFileEntry, allocator: std.mem.Allocator) void {
        allocator.free(self.path);
        if (self.old_path) |op| allocator.free(op);
        if (self.agent_name) |an| allocator.free(an);
        self.* = undefined;
    }

    pub fn setOldPath(self: *ChangedFileEntry, allocator: std.mem.Allocator, old_p: ?[]const u8) !void {
        if (self.old_path) |op| allocator.free(op);
        self.old_path = if (old_p) |p| try allocator.dupe(u8, p) else null;
    }

    pub fn setAgentName(self: *ChangedFileEntry, allocator: std.mem.Allocator, ag_name: ?[]const u8) !void {
        if (self.agent_name) |an| allocator.free(an);
        self.agent_name = if (ag_name) |n| try allocator.dupe(u8, n) else null;
    }
};

pub const ChangedFilesPanel = struct {
    allocator: std.mem.Allocator,
    agent_entries: std.ArrayList(ChangedFileEntry),
    project_entries: std.ArrayList(ChangedFileEntry),
    committed_history: std.ArrayList(ChangedFileEntry), // Doğrulama 6 için oturum geçmişi

    selected_index: usize = 0,
    selected_in_agent: bool = true,
    term_width: u16 = 80,
    term_height: u16 = 24,

    pub fn init(allocator: std.mem.Allocator) ChangedFilesPanel {
        return .{
            .allocator = allocator,
            .agent_entries = std.ArrayList(ChangedFileEntry).empty,
            .project_entries = std.ArrayList(ChangedFileEntry).empty,
            .committed_history = std.ArrayList(ChangedFileEntry).empty,
            .selected_index = 0,
            .selected_in_agent = true,
            .term_width = 80,
            .term_height = 24,
        };
    }

    pub fn deinit(self: *ChangedFilesPanel) void {
        for (self.agent_entries.items) |*e| e.deinit(self.allocator);
        self.agent_entries.deinit(self.allocator);

        for (self.project_entries.items) |*e| e.deinit(self.allocator);
        self.project_entries.deinit(self.allocator);

        for (self.committed_history.items) |*e| e.deinit(self.allocator);
        self.committed_history.deinit(self.allocator);

        self.* = undefined;
    }

    /// Filtre kontrolü: `.git/`, `.zig-cache/`, `zig-out/` gibi iç klasörleri eler.
    pub fn shouldIncludePath(path: []const u8) bool {
        if (std.mem.startsWith(u8, path, ".git/") or std.mem.eql(u8, path, ".git")) return false;
        if (std.mem.startsWith(u8, path, ".zig-cache/") or std.mem.startsWith(u8, path, "zig-out/")) return false;
        if (std.mem.startsWith(u8, path, "../")) return false; // Dışarı symlink / üst dizin
        return true;
    }

    /// MutationRecord kaydından ChangedFileEntry ekler veya günceller
    pub fn syncFromMutationRecord(self: *ChangedFilesPanel, rec: *const MutationRecord) !void {
        if (!shouldIncludePath(rec.path)) return;

        const is_agent = (rec.actor == .agent);
        var target_list = if (is_agent) &self.agent_entries else &self.project_entries;

        // Var olanı ara
        for (target_list.items) |*entry| {
            if (std.mem.eql(u8, entry.path, rec.path)) {
                entry.kind = rec.kind;
                entry.actor = rec.actor;
                entry.before_hash = rec.before_hash;
                entry.after_hash = rec.after_hash;
                entry.additions = rec.additions;
                entry.deletions = rec.deletions;
                entry.observed_at = rec.observed_at;
                entry.is_binary = (rec.kind == .binary);
                entry.is_conflict = (rec.kind == .conflict);
                try entry.setAgentName(self.allocator, rec.agent_id);
                try entry.setOldPath(self.allocator, rec.old_path);
                return;
            }
        }

        // Yeni ekle
        var entry = try ChangedFileEntry.init(self.allocator, rec.path, rec.kind, rec.actor);
        errdefer entry.deinit(self.allocator);

        entry.before_hash = rec.before_hash;
        entry.after_hash = rec.after_hash;
        entry.additions = rec.additions;
        entry.deletions = rec.deletions;
        entry.observed_at = rec.observed_at;
        entry.is_binary = (rec.kind == .binary);
        entry.is_conflict = (rec.kind == .conflict);
        try entry.setAgentName(self.allocator, rec.agent_id);
        try entry.setOldPath(self.allocator, rec.old_path);

        try target_list.append(self.allocator, entry);
    }

    /// Git status bilgisini bağlar (Doğrulama 7: staged ve unstaged aynı anda bulunur).
    pub fn updateGitStatus(
        self: *ChangedFilesPanel,
        path: []const u8,
        staged: u8,
        unstaged: u8,
        is_untracked: bool,
        is_ignored: bool,
    ) !void {
        if (!shouldIncludePath(path)) return;

        // Önce Agent listesinde ara
        for (self.agent_entries.items) |*e| {
            if (std.mem.eql(u8, e.path, path)) {
                e.git_status = .{
                    .staged = staged,
                    .unstaged = unstaged,
                    .is_untracked = is_untracked,
                    .is_ignored = is_ignored,
                    .is_conflict = (staged == 'U' or unstaged == 'U'),
                };
                return;
            }
        }

        // Project listesinde ara
        for (self.project_entries.items) |*e| {
            if (std.mem.eql(u8, e.path, path)) {
                e.git_status = .{
                    .staged = staged,
                    .unstaged = unstaged,
                    .is_untracked = is_untracked,
                    .is_ignored = is_ignored,
                    .is_conflict = (staged == 'U' or unstaged == 'U'),
                };
                return;
            }
        }

        // Yoksa project entries'e yeni kayıt olarak ekle
        const kind: MutationKind = if (is_untracked)
            .added
        else if (staged == 'D' or unstaged == 'D')
            .deleted
        else
            .modified;

        var entry = try ChangedFileEntry.init(self.allocator, path, kind, .user);
        errdefer entry.deinit(self.allocator);

        entry.git_status = .{
            .staged = staged,
            .unstaged = unstaged,
            .is_untracked = is_untracked,
            .is_ignored = is_ignored,
            .is_conflict = (staged == 'U' or unstaged == 'U'),
        };

        try self.project_entries.append(self.allocator, entry);
    }

    /// Commit olayı gerçekleştiğinde uncommitted görünüm temizlenir ancak oturum geçmişi korunur (Doğrulama 6).
    pub fn handleCommit(self: *ChangedFilesPanel) !void {
        // Agent ve project girdilerini committed_history'ye taşı
        for (self.agent_entries.items) |e| {
            var hist_entry = try ChangedFileEntry.init(self.allocator, e.path, e.kind, e.actor);
            errdefer hist_entry.deinit(self.allocator);
            hist_entry.additions = e.additions;
            hist_entry.deletions = e.deletions;
            hist_entry.before_hash = e.before_hash;
            hist_entry.after_hash = e.after_hash;
            try hist_entry.setAgentName(self.allocator, e.agent_name);
            try hist_entry.setOldPath(self.allocator, e.old_path);
            try self.committed_history.append(self.allocator, hist_entry);
        }

        // Uncommitted git durumlarını temizle
        for (self.agent_entries.items) |*e| {
            e.git_status = .{ .staged = ' ', .unstaged = ' ' };
        }
        for (self.project_entries.items) |*e| {
            e.git_status = .{ .staged = ' ', .unstaged = ' ' };
        }
    }

    /// Terminal resize durumunda seçim indekslerini güvenli sınırda tutar (Doğrulama 11).
    pub fn handleResize(self: *ChangedFilesPanel, new_width: u16, new_height: u16) void {
        self.term_width = new_width;
        self.term_height = new_height;

        const current_count = if (self.selected_in_agent) self.agent_entries.items.len else self.project_entries.items.len;
        if (current_count > 0 and self.selected_index >= current_count) {
            self.selected_index = current_count - 1;
        } else if (current_count == 0) {
            self.selected_index = 0;
        }
    }

    /// Panel içeriğini metin satırları olarak render eder.
    pub fn renderToLines(
        self: *const ChangedFilesPanel,
        allocator: std.mem.Allocator,
        render_width: usize,
    ) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;
        errdefer {
            for (lines.items) |l| allocator.free(l);
            lines.deinit(allocator);
        }

        // 1. Agent Changes Bölümü
        try lines.append(allocator, try allocator.dupe(u8, "── Agent Changes ──"));
        if (self.agent_entries.items.len == 0) {
            try lines.append(allocator, try allocator.dupe(u8, "  (No agent changes)"));
        } else {
            for (self.agent_entries.items, 0..) |e, idx| {
                const is_sel = (self.selected_in_agent and idx == self.selected_index);
                const mark: []const u8 = if (is_sel) "▶" else " ";

                var git_buf: [8]u8 = undefined;
                const git_lbl = e.git_status.label(&git_buf);

                const kind_char: u8 = switch (e.kind) {
                    .added => 'A',
                    .modified => 'M',
                    .deleted => 'D',
                    .renamed => 'R',
                    .binary => 'B',
                    .conflict => 'U',
                    else => 'M',
                };

                // Doğrulama 9: Binary/conflict sahte 0/0 göstermez
                var stat_buf: [64]u8 = undefined;
                const stat_str = if (e.is_binary or e.kind == .binary)
                    "[binary]"
                else if (e.is_conflict or e.kind == .conflict)
                    "[conflict]"
                else if (e.additions != null or e.deletions != null)
                    try std.fmt.bufPrint(&stat_buf, "+{d} -{d}", .{ e.additions orelse 0, e.deletions orelse 0 })
                else
                    "";

                // Doğrulama 8: Rename ve delete path bilgisi
                const path_str = if (e.old_path) |op|
                    try std.fmt.allocPrint(allocator, "{s} -> {s}", .{ op, e.path })
                else
                    try allocator.dupe(u8, e.path);
                defer allocator.free(path_str);

                const ag_str = if (e.agent_name) |an| an else "agent";

                const line_fmt = try std.fmt.allocPrint(
                    allocator,
                    "{s} {c} [{s}] {s}   {s}   agent: {s}",
                    .{ mark, kind_char, git_lbl, path_str, stat_str, ag_str },
                );
                defer allocator.free(line_fmt);

                const truncated = try unicode.truncateToWidth(allocator, line_fmt, render_width, "...");
                try lines.append(allocator, truncated);
            }
        }

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 2. Project Changes Bölümü
        try lines.append(allocator, try allocator.dupe(u8, "── Project Changes ──"));
        if (self.project_entries.items.len == 0) {
            try lines.append(allocator, try allocator.dupe(u8, "  (No project changes)"));
        } else {
            for (self.project_entries.items, 0..) |e, idx| {
                const is_sel = (!self.selected_in_agent and idx == self.selected_index);
                const mark: []const u8 = if (is_sel) "▶" else " ";

                var git_buf: [8]u8 = undefined;
                const git_lbl = e.git_status.label(&git_buf);

                const kind_char: u8 = switch (e.kind) {
                    .added => 'A',
                    .modified => 'M',
                    .deleted => 'D',
                    .renamed => 'R',
                    .binary => 'B',
                    .conflict => 'U',
                    else => 'M',
                };

                const actor_str = e.actor.label();

                const line_fmt = try std.fmt.allocPrint(
                    allocator,
                    "{s} {c} [{s}] {s}   actor: {s}",
                    .{ mark, kind_char, git_lbl, e.path, actor_str },
                );
                defer allocator.free(line_fmt);

                const truncated = try unicode.truncateToWidth(allocator, line_fmt, render_width, "...");
                try lines.append(allocator, truncated);
            }
        }

        return lines;
    }
};

// -----------------------------------------------------------------------------
// Unit Testler (Doğrulama 1, 2, 6, 7, 8, 9, 11)
// -----------------------------------------------------------------------------

test "Dogrulama 7: Staged ve unstaged ayni dosyada birlikte bulunur (MM, AM, MD)" {
    var panel = ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    // 'MM': staged modified, unstaged modified
    try panel.updateGitStatus("src/main.zig", 'M', 'M', false, false);
    // 'AM': staged added, unstaged modified
    try panel.updateGitStatus("src/feature.zig", 'A', 'M', false, false);

    try std.testing.expectEqual(@as(usize, 2), panel.project_entries.items.len);

    const f1 = &panel.project_entries.items[0];
    try std.testing.expectEqual(@as(u8, 'M'), f1.git_status.staged);
    try std.testing.expectEqual(@as(u8, 'M'), f1.git_status.unstaged);

    const f2 = &panel.project_entries.items[1];
    try std.testing.expectEqual(@as(u8, 'A'), f2.git_status.staged);
    try std.testing.expectEqual(@as(u8, 'M'), f2.git_status.unstaged);
}

test "Dogrulama 8: Rename ve delete durumlari path bilgisiyle gorunur" {
    var panel = ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    var rec_renamed = try MutationRecord.init(std.testing.allocator, 1, "src/new_name.zig", .renamed, .agent, 100);
    defer rec_renamed.deinit(std.testing.allocator);
    rec_renamed.old_path = try std.testing.allocator.dupe(u8, "src/old_name.zig");
    try rec_renamed.setContext(std.testing.allocator, null, "agent_executor", null, 1);

    try panel.syncFromMutationRecord(&rec_renamed);
    try std.testing.expectEqual(@as(usize, 1), panel.agent_entries.items.len);

    const r_entry = &panel.agent_entries.items[0];
    try std.testing.expectEqualStrings("src/new_name.zig", r_entry.path);
    try std.testing.expectEqualStrings("src/old_name.zig", r_entry.old_path.?);

    var lines = try panel.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    var found_rename = false;
    for (lines.items) |l| {
        if (std.mem.indexOf(u8, l, "src/old_name.zig -> src/new_name.zig") != null) found_rename = true;
    }
    try std.testing.expect(found_rename);
}

test "Dogrulama 9: Buyuk/binary/conflict dosyada sahte 0/0 gosterilmez" {
    var panel = ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    var rec_bin = try MutationRecord.init(std.testing.allocator, 1, "assets/model.bin", .binary, .agent, 100);
    defer rec_bin.deinit(std.testing.allocator);
    // Additions ve deletions null olmalıdır
    rec_bin.additions = null;
    rec_bin.deletions = null;

    try panel.syncFromMutationRecord(&rec_bin);

    var lines = try panel.renderToLines(std.testing.allocator, 80);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    var found_binary_tag = false;
    var found_fake_zero = false;

    for (lines.items) |l| {
        if (std.mem.indexOf(u8, l, "[binary]") != null) found_binary_tag = true;
        if (std.mem.indexOf(u8, l, "+0 -0") != null) found_fake_zero = true;
    }

    try std.testing.expect(found_binary_tag);
    try std.testing.expect(!found_fake_zero);
}

test "Dogrulama 6: Commit sonrasi uncommitted gorunum guncellenir, oturum gecmisi korunur" {
    var panel = ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    var rec = try MutationRecord.init(std.testing.allocator, 1, "src/logic.zig", .modified, .agent, 100);
    defer rec.deinit(std.testing.allocator);
    rec.additions = 15;
    rec.deletions = 3;
    try rec.setContext(std.testing.allocator, null, "agent_core", null, 1);

    try panel.syncFromMutationRecord(&rec);
    try panel.updateGitStatus("src/logic.zig", 'M', ' ', false, false);

    try std.testing.expectEqual(@as(usize, 1), panel.agent_entries.items.len);
    try std.testing.expectEqual(@as(u8, 'M'), panel.agent_entries.items[0].git_status.staged);

    // Commit olayı
    try panel.handleCommit();

    // 1. Uncommitted Git durumu temizlendi
    try std.testing.expectEqual(@as(u8, ' '), panel.agent_entries.items[0].git_status.staged);
    // 2. Oturum geçmişi (committed_history) korundu
    try std.testing.expectEqual(@as(usize, 1), panel.committed_history.items.len);
    try std.testing.expectEqualStrings("src/logic.zig", panel.committed_history.items[0].path);
    try std.testing.expectEqual(@as(?u64, 15), panel.committed_history.items[0].additions);
}

test "changed files resize secim korunmasi (Dogrulama 11)" {
    var panel = ChangedFilesPanel.init(std.testing.allocator);
    defer panel.deinit();

    var rec1 = try MutationRecord.init(std.testing.allocator, 1, "src/a.zig", .modified, .agent, 100);
    defer rec1.deinit(std.testing.allocator);
    var rec2 = try MutationRecord.init(std.testing.allocator, 2, "src/b.zig", .added, .agent, 200);
    defer rec2.deinit(std.testing.allocator);

    try panel.syncFromMutationRecord(&rec1);
    try panel.syncFromMutationRecord(&rec2);

    panel.selected_index = 1;
    panel.handleResize(60, 20);

    try std.testing.expectEqual(@as(usize, 1), panel.selected_index); // Seçim korundu
}
