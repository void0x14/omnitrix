//! Omnitrix — Zig runtime çekirdeği.
//!
//! Onaylı tasarım: docs/superpowers/specs/2026-08-17-omnitrix-zig-runtime-change-ledger-design.md
//! Bu modül, tek-süreç Omnitrix runtime'ının ortak sözleşmelerini ve çekirdek
//! modüllerini barındırır: omnitrix-io, omnitrix-task, omnitrix-net, omnitrix-stream,
//! omnitrix-permission (Kol C), FileMutationLedger ve omnitrix-runtime (StateStore).
//! Modüller arasında worker süreci, JSON/Protobuf IPC veya token başına serialization
//! yoktur (tasarım Bölüm 1, 4).
//!
//! Zig 0.17.0-dev notu: `std.fs` kaldırıldı; dosya işlemleri `std.Io` modülündedir.
//! omnitrix-io, upstream `std.Io.Evented` durumuna bağımlı DEĞİLDİR (tasarım 3.1).

const std = @import("std");

pub const io = @import("omnitrix-io/io.zig");
pub const task = @import("omnitrix-task/task.zig");
pub const real_pty = @import("omnitrix-task/real_pty.zig");
pub const net = @import("omnitrix-net/net.zig");
pub const stream = @import("omnitrix-stream/stream.zig");

// Kol C: Permission & Gerçeklik Kapısı
pub const permission = @import("omnitrix-permission/permission.zig");
pub const capability = @import("omnitrix-permission/capability.zig");
pub const doom_loop = @import("omnitrix-permission/doom_loop.zig");
pub const broker = @import("omnitrix-permission/broker.zig");

// FileMutationLedger & İlgili Modüller
pub const ledger = @import("omnitrix-ledger/ledger.zig");
pub const mutation = @import("omnitrix-ledger/mutation.zig");
pub const hash = @import("omnitrix-ledger/hash.zig");
pub const scan = @import("omnitrix-ledger/scan.zig");
pub const tool_harness = @import("omnitrix-ledger/tool_harness.zig");
pub const shell_snapshot = @import("omnitrix-ledger/shell_snapshot.zig");
pub const hunk = @import("omnitrix-ledger/hunk.zig");
pub const attribution = @import("omnitrix-ledger/attribution.zig");
pub const revert = @import("omnitrix-ledger/revert.zig");
pub const watcher_debounce = @import("omnitrix-ledger/watcher_debounce.zig");
pub const kol_c_verification_test = @import("omnitrix-ledger/kol_c_verification_test.zig");

pub const runtime = @import("omnitrix-runtime/runtime.zig");
pub const metrics = runtime.metrics;
pub const soak = runtime.soak;
pub const faz9_verification_test = @import("omnitrix-runtime/soak.zig");

// Kol A: TUI & 2D Engine (Bölüm 5)
pub const tui = @import("omnitrix-tui/tui.zig");
pub const terminal = @import("omnitrix-tui/terminal.zig");
pub const unicode = @import("omnitrix-tui/unicode.zig");
pub const theme = @import("omnitrix-tui/theme.zig");
pub const goal = @import("omnitrix-tui/goal.zig");
pub const voice = @import("omnitrix-tui/voice.zig");
pub const input_box = @import("omnitrix-tui/input_box.zig");
pub const sidebar = @import("omnitrix-tui/sidebar.zig");
pub const question_view = @import("omnitrix-tui/question_view.zig");
pub const block_renderer = @import("omnitrix-tui/block_renderer.zig");
pub const diff_renderer = @import("omnitrix-tui/diff_renderer.zig");
pub const changed_files = @import("omnitrix-tui/changed_files.zig");
pub const pty_harness = @import("omnitrix-tui/pty_harness.zig");
pub const kol_a_verification_test = @import("omnitrix-tui/kol_a_verification_test.zig");

// 2D Core TUI Engine & Input/Editor Subsystems
pub const tui_keys = @import("omnitrix-tui/input/keys.zig");
pub const tui_input_parser = @import("omnitrix-tui/input/parser.zig");
pub const tui_gap_buffer = @import("omnitrix-tui/editor/gap_buffer.zig");
pub const tui_prompt_editor = @import("omnitrix-tui/editor/prompt_editor.zig");
pub const tui_cell = @import("omnitrix-tui/core/cell.zig");
pub const tui_geometry = @import("omnitrix-tui/core/geometry.zig");
pub const tui_buffer = @import("omnitrix-tui/core/buffer.zig");
pub const tui_layout = @import("omnitrix-tui/core/layout.zig");
pub const tui_diff = @import("omnitrix-tui/core/diff.zig");
pub const tui_block = @import("omnitrix-tui/widgets/block.zig");
pub const tui_paragraph = @import("omnitrix-tui/widgets/paragraph.zig");
pub const tui_list = @import("omnitrix-tui/widgets/list.zig");
pub const tui_tabs = @import("omnitrix-tui/widgets/tabs.zig");
pub const tui_textarea = @import("omnitrix-tui/widgets/textarea.zig");
pub const tui_header_view = @import("omnitrix-tui/views/header_view.zig");
pub const tui_sidebar_view = @import("omnitrix-tui/views/sidebar_view.zig");
pub const tui_question_view = @import("omnitrix-tui/views/question_view.zig");
pub const tui_engine = @import("omnitrix-tui/runtime/engine.zig");

test {
    std.testing.refAllDecls(@This());
}
