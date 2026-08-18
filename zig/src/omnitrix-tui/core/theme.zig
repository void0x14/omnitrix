const std = @import("std");
const cell_mod = @import("cell.zig");
const Color = cell_mod.Color;
const Style = cell_mod.Style;
const Attributes = cell_mod.Attributes;

pub const ThemeMode = enum { dark, light };

pub const Theme = struct {
    background: Color,
    foreground: Color,
    background_panel: Color,
    background_elevated: Color,
    border: Color,
    border_active: Color,
    border_muted: Color,
    text: Color,
    text_muted: Color,
    text_dim: Color,
    accent: Color,
    accent_muted: Color,
    success: Color,
    success_muted: Color,
    warning: Color,
    warning_muted: Color,
    err_color: Color,
    err_muted: Color,
    info: Color,
    info_muted: Color,
    // Syntax
    syntax_keyword: Color,
    syntax_string: Color,
    syntax_number: Color,
    syntax_comment: Color,
    syntax_function: Color,
    syntax_type: Color,
    syntax_variable: Color,
    syntax_operator: Color,
    syntax_delimiter: Color,
    syntax_constant: Color,
    // Diff
    diff_add_bg: Color,
    diff_add_fg: Color,
    diff_del_bg: Color,
    diff_del_fg: Color,
    diff_hunk_bg: Color,
    diff_hunk_fg: Color,
    // Scrollbar
    scrollbar_track: Color,
    scrollbar_thumb: Color,
    // Prompt
    prompt_placeholder: Color,
    prompt_cursor: Color,

    pub const dark: Theme = .{
        .background = .{ .rgb = .{ .r = 13, .g = 13, .b = 13 } },
        .foreground = .{ .rgb = .{ .r = 212, .g = 212, .b = 212 } },
        .background_panel = .{ .rgb = .{ .r = 22, .g = 22, .b = 22 } },
        .background_elevated = .{ .rgb = .{ .r = 30, .g = 30, .b = 30 } },
        .border = .{ .rgb = .{ .r = 55, .g = 55, .b = 55 } },
        .border_active = .{ .rgb = .{ .r = 100, .g = 100, .b = 255 } },
        .border_muted = .{ .rgb = .{ .r = 40, .g = 40, .b = 40 } },
        .text = .{ .rgb = .{ .r = 212, .g = 212, .b = 212 } },
        .text_muted = .{ .rgb = .{ .r = 130, .g = 130, .b = 130 } },
        .text_dim = .{ .rgb = .{ .r = 90, .g = 90, .b = 90 } },
        .accent = .{ .rgb = .{ .r = 120, .g = 140, .b = 255 } },
        .accent_muted = .{ .rgb = .{ .r = 80, .g = 95, .b = 180 } },
        .success = .{ .rgb = .{ .r = 80, .g = 200, .b = 120 } },
        .success_muted = .{ .rgb = .{ .r = 50, .g = 120, .b = 75 } },
        .warning = .{ .rgb = .{ .r = 255, .g = 200, .b = 50 } },
        .warning_muted = .{ .rgb = .{ .r = 180, .g = 140, .b = 40 } },
        .err_color = .{ .rgb = .{ .r = 255, .g = 80, .b = 80 } },
        .err_muted = .{ .rgb = .{ .r = 180, .g = 55, .b = 55 } },
        .info = .{ .rgb = .{ .r = 100, .g = 180, .b = 255 } },
        .info_muted = .{ .rgb = .{ .r = 60, .g = 110, .b = 180 } },
        .syntax_keyword = .{ .rgb = .{ .r = 198, .g = 120, .b = 221 } },
        .syntax_string = .{ .rgb = .{ .r = 152, .g = 195, .b = 121 } },
        .syntax_number = .{ .rgb = .{ .r = 209, .g = 154, .b = 102 } },
        .syntax_comment = .{ .rgb = .{ .r = 92, .g = 99, .b = 112 } },
        .syntax_function = .{ .rgb = .{ .r = 97, .g = 175, .b = 239 } },
        .syntax_type = .{ .rgb = .{ .r = 229, .g = 192, .b = 123 } },
        .syntax_variable = .{ .rgb = .{ .r = 224, .g = 108, .b = 117 } },
        .syntax_operator = .{ .rgb = .{ .r = 86, .g = 182, .b = 194 } },
        .syntax_delimiter = .{ .rgb = .{ .r = 171, .g = 178, .b = 191 } },
        .syntax_constant = .{ .rgb = .{ .r = 209, .g = 154, .b = 102 } },
        .diff_add_bg = .{ .rgb = .{ .r = 30, .g = 50, .b = 30 } },
        .diff_add_fg = .{ .rgb = .{ .r = 152, .g = 195, .b = 121 } },
        .diff_del_bg = .{ .rgb = .{ .r = 50, .g = 30, .b = 30 } },
        .diff_del_fg = .{ .rgb = .{ .r = 224, .g = 108, .b = 117 } },
        .diff_hunk_bg = .{ .rgb = .{ .r = 30, .g = 30, .b = 50 } },
        .diff_hunk_fg = .{ .rgb = .{ .r = 198, .g = 120, .b = 221 } },
        .scrollbar_track = .{ .rgb = .{ .r = 30, .g = 30, .b = 30 } },
        .scrollbar_thumb = .{ .rgb = .{ .r = 80, .g = 80, .b = 80 } },
        .prompt_placeholder = .{ .rgb = .{ .r = 90, .g = 90, .b = 90 } },
        .prompt_cursor = .{ .rgb = .{ .r = 120, .g = 140, .b = 255 } },
    };

    pub const light: Theme = .{
        .background = .{ .rgb = .{ .r = 255, .g = 255, .b = 255 } },
        .foreground = .{ .rgb = .{ .r = 38, .g = 38, .b = 38 } },
        .background_panel = .{ .rgb = .{ .r = 245, .g = 245, .b = 245 } },
        .background_elevated = .{ .rgb = .{ .r = 240, .g = 240, .b = 240 } },
        .border = .{ .rgb = .{ .r = 200, .g = 200, .b = 200 } },
        .border_active = .{ .rgb = .{ .r = 60, .g = 80, .b = 220 } },
        .border_muted = .{ .rgb = .{ .r = 220, .g = 220, .b = 220 } },
        .text = .{ .rgb = .{ .r = 38, .g = 38, .b = 38 } },
        .text_muted = .{ .rgb = .{ .r = 120, .g = 120, .b = 120 } },
        .text_dim = .{ .rgb = .{ .r = 170, .g = 170, .b = 170 } },
        .accent = .{ .rgb = .{ .r = 60, .g = 80, .b = 220 } },
        .accent_muted = .{ .rgb = .{ .r = 90, .g = 110, .b = 200 } },
        .success = .{ .rgb = .{ .r = 40, .g = 160, .b = 80 } },
        .success_muted = .{ .rgb = .{ .r = 70, .g = 180, .b = 110 } },
        .warning = .{ .rgb = .{ .r = 200, .g = 150, .b = 0 } },
        .warning_muted = .{ .rgb = .{ .r = 180, .g = 140, .b = 30 } },
        .err_color = .{ .rgb = .{ .r = 220, .g = 50, .b = 50 } },
        .err_muted = .{ .rgb = .{ .r = 200, .g = 80, .b = 80 } },
        .info = .{ .rgb = .{ .r = 40, .g = 120, .b = 220 } },
        .info_muted = .{ .rgb = .{ .r = 70, .g = 140, .b = 200 } },
        .syntax_keyword = .{ .rgb = .{ .r = 160, .g = 60, .b = 180 } },
        .syntax_string = .{ .rgb = .{ .r = 40, .g = 140, .b = 40 } },
        .syntax_number = .{ .rgb = .{ .r = 180, .g = 100, .b = 20 } },
        .syntax_comment = .{ .rgb = .{ .r = 140, .g = 140, .b = 150 } },
        .syntax_function = .{ .rgb = .{ .r = 40, .g = 100, .b = 200 } },
        .syntax_type = .{ .rgb = .{ .r = 180, .g = 130, .b = 20 } },
        .syntax_variable = .{ .rgb = .{ .r = 190, .g = 50, .b = 50 } },
        .syntax_operator = .{ .rgb = .{ .r = 20, .g = 140, .b = 160 } },
        .syntax_delimiter = .{ .rgb = .{ .r = 80, .g = 80, .b = 80 } },
        .syntax_constant = .{ .rgb = .{ .r = 180, .g = 100, .b = 20 } },
        .diff_add_bg = .{ .rgb = .{ .r = 220, .g = 255, .b = 220 } },
        .diff_add_fg = .{ .rgb = .{ .r = 40, .g = 140, .b = 40 } },
        .diff_del_bg = .{ .rgb = .{ .r = 255, .g = 220, .b = 220 } },
        .diff_del_fg = .{ .rgb = .{ .r = 190, .g = 50, .b = 50 } },
        .diff_hunk_bg = .{ .rgb = .{ .r = 220, .g = 220, .b = 255 } },
        .diff_hunk_fg = .{ .rgb = .{ .r = 160, .g = 60, .b = 180 } },
        .scrollbar_track = .{ .rgb = .{ .r = 230, .g = 230, .b = 230 } },
        .scrollbar_thumb = .{ .rgb = .{ .r = 170, .g = 170, .b = 170 } },
        .prompt_placeholder = .{ .rgb = .{ .r = 170, .g = 170, .b = 170 } },
        .prompt_cursor = .{ .rgb = .{ .r = 60, .g = 80, .b = 220 } },
    };

    pub fn get(mode: ThemeMode) Theme {
        return switch (mode) {
            .dark => dark,
            .light => light,
        };
    }

    pub fn fgStyle(self: Theme, color: Color) Style {
        return .{ .fg = color, .bg = self.background };
    }

    pub fn panelStyle(self: Theme, color: Color) Style {
        return .{ .fg = color, .bg = self.background_panel };
    }

    pub fn boldStyle(self: Theme, color: Color) Style {
        return .{ .fg = color, .bg = self.background, .attr = .{ .bold = true } };
    }

    pub fn mutedStyle(self: Theme) Style {
        return .{ .fg = self.text_muted, .bg = self.background };
    }

    pub fn dimStyle(self: Theme) Style {
        return .{ .fg = self.text_dim, .bg = self.background };
    }

    pub fn panelMutedStyle(self: Theme) Style {
        return .{ .fg = self.text_muted, .bg = self.background_panel };
    }

    pub fn panelBoldStyle(self: Theme, color: Color) Style {
        return .{ .fg = color, .bg = self.background_panel, .attr = .{ .bold = true } };
    }
};
