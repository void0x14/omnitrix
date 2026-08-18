//! omnitrix-tui: Tema, Renkler ve Kutu Çizim Karakterleri (OpenCode & Grok Stili)
//!
//! Şartname: OpenCode TUI, Grok Voice UI, Codex Goal Tracker

const std = @import("std");
const term = @import("terminal.zig");

pub const Color = term.Color;
pub const Style = term.Style;
pub const ANSI = term.ANSI;

/// Kutu çizim karakterleri (Rounded & Sharp Unicode)
pub const Box = struct {
    // Rounded köşeler (OpenCode varsayılanı)
    pub const top_left = "╭";
    pub const top_right = "╮";
    pub const bottom_left = "╰";
    pub const bottom_right = "╯";
    pub const horizontal = "─";
    pub const vertical = "│";
    pub const t_down = "┬";
    pub const t_up = "┴";
    pub const t_right = "├";
    pub const t_left = "┤";
    pub const cross = "┼";

    // Keskin köşeler
    pub const sharp_top_left = "┌";
    pub const sharp_top_right = "┐";
    pub const sharp_bottom_left = "└";
    pub const sharp_bottom_right = "┘";

    // Progress bar karakterleri
    pub const prog_full = "█";
    pub const prog_7_8 = "▉";
    pub const prog_3_4 = "▊";
    pub const prog_5_8 = "▋";
    pub const prog_1_2 = "▌";
    pub const prog_3_8 = "▍";
    pub const prog_1_4 = "▎";
    pub const prog_1_8 = "▏";
    pub const prog_empty = "░";
};

/// İkonlar & Göstergeler
pub const Icons = struct {
    pub const omnitrix = "✦";
    pub const bolt = "⚡";
    pub const bot = "🤖";
    pub const user = "👤";
    pub const system = "⚙️";
    pub const tool = "🛠️";
    pub const goal = "🎯";
    pub const voice = "🎙️";
    pub const file = "📄";
    pub const folder = "📁";
    pub const git = "🌿";
    pub const check = "✓";
    pub const cross = "✗";
    pub const active = "▶";
    pub const pending = "○";
    pub const prompt = "❯";
    pub const dot_green = "🟢";
    pub const dot_yellow = "🟡";
    pub const dot_red = "🔴";
};

/// Tema Renk Paletleri
pub const Theme = struct {
    pub const border = Color{ .ansi = 239 }; // Koyu gri kenarlık
    pub const border_focused = Color{ .ansi = 75 }; // Canlı mavi odak kenarlığı
    pub const bg_dark = Color{ .ansi = 234 };
    pub const header_bg = Color{ .ansi = 236 };
    pub const status_bg = Color{ .ansi = 235 };

    pub const text_main = Color{ .ansi = 254 };
    pub const text_dim = Color{ .ansi = 244 };
    pub const text_cyan = Color{ .ansi = 81 };
    pub const text_green = Color{ .ansi = 120 };
    pub const text_yellow = Color{ .ansi = 221 };
    pub const text_magenta = Color{ .ansi = 207 };
    pub const text_red = Color{ .ansi = 203 };

    // Kart başlık stilleri
    pub const header_system = Style{ .fg = Color{ .ansi = 247 }, .bold = true };
    pub const header_user = Style{ .fg = Color{ .ansi = 81 }, .bold = true };
    pub const header_agent = Style{ .fg = Color{ .ansi = 120 }, .bold = true };
    pub const header_tool = Style{ .fg = Color{ .ansi = 221 }, .bold = true };
    pub const header_goal = Style{ .fg = Color{ .ansi = 207 }, .bold = true };
    pub const header_voice = Style{ .fg = Color{ .ansi = 117 }, .bold = true };
};
