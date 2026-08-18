//! omnitrix-tui/dialogs/modal.zig
//!
//! Z-Index tabanlı Yüzen Modal ve Diyalog Katman Yöneticisi (Overlay / Modal Manager).
//! Ekranın üzerine arka plan karartmalı (backdrop dim), gölgeli ve odak yakalamalı (focus trap)
//! bağımsız pencereler açılmasını sağlar.

const std = @import("std");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");
const cell_mod = @import("../core/cell.zig");
const keys_mod = @import("../input/keys.zig");

pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;
pub const Style = cell_mod.Style;
pub const Color = cell_mod.Color;
pub const KeyEvent = keys_mod.KeyEvent;

pub const ModalConfig = struct {
    title: []const u8,
    width_pct: u16 = 60,
    height_pct: u16 = 50,
    min_width: u16 = 40,
    min_height: u16 = 10,
    dim_backdrop: bool = true,
    border_style: Style = .{ .fg = .{ .indexed = 39 } },
    title_style: Style = .{ .fg = .bright_white, .modifier = .{ .bold = true } },
};

pub const Modal = struct {
    config: ModalConfig,
    area: Rect = Rect.zero,

    pub fn init(config: ModalConfig) Modal {
        return .{
            .config = config,
            .area = Rect.zero,
        };
    }

    /// Ekran alanına göre modalın merkezlenmiş konumunu hesaplar.
    pub fn calculateArea(self: *Modal, screen_area: Rect) Rect {
        if (screen_area.isEmpty()) return Rect.zero;

        const calc_w = @max(self.config.min_width, (screen_area.width * self.config.width_pct) / 100);
        const calc_h = @max(self.config.min_height, (screen_area.height * self.config.height_pct) / 100);

        const w = @min(calc_w, screen_area.width);
        const h = @min(calc_h, screen_area.height);

        const x = screen_area.left() + (screen_area.width - w) / 2;
        const y = screen_area.top() + (screen_area.height - h) / 2;

        self.area = Rect.init(x, y, w, h);
        return self.area;
    }

    /// Modal penceresini arka plan karartması, kenarlık ve başlığıyla çizer.
    /// İçerik alanı (inner rect) döner.
    pub fn renderFrame(self: *const Modal, screen_area: Rect, buf: *Buffer) Rect {
        if (self.area.isEmpty()) return Rect.zero;

        // 1. Arka Planı Karart (Dim Backdrop)
        if (self.config.dim_backdrop) {
            var by = screen_area.top();
            while (by < screen_area.bottom()) : (by += 1) {
                var bx = screen_area.left();
                while (bx < screen_area.right()) : (bx += 1) {
                    if (buf.getMut(bx, by)) |c| {
                        var st = c.style;
                        st.modifier.dim = true;
                        st.fg = .{ .indexed = 240 };
                        c.setStyle(st);
                    }
                }
            }
        }

        // 2. Modal Gövdesini Temizle (Koyu Arka Plan)
        const bg_style = Style{ .fg = .bright_white, .bg = .{ .indexed = 234 } };
        var my = self.area.top();
        while (my < self.area.bottom()) : (my += 1) {
            var mx = self.area.left();
            while (mx < self.area.right()) : (mx += 1) {
                if (buf.getMut(mx, my)) |c| {
                    c.setSymbol(" ", 1);
                    c.setStyle(bg_style);
                }
            }
        }

        // 3. Kenarlık Çiz (Yuvarlatılmış Köşeler: ╭ ╮ ╰ ╯ ─ │)
        const border_style = self.config.border_style;
        const left = self.area.left();
        const right = self.area.right() - 1;
        const top = self.area.top();
        const bottom = self.area.bottom() - 1;

        // Köşeler
        if (buf.getMut(left, top)) |c| { c.setSymbol("╭", 1); c.setStyle(border_style); }
        if (buf.getMut(right, top)) |c| { c.setSymbol("╮", 1); c.setStyle(border_style); }
        if (buf.getMut(left, bottom)) |c| { c.setSymbol("╰", 1); c.setStyle(border_style); }
        if (buf.getMut(right, bottom)) |c| { c.setSymbol("╯", 1); c.setStyle(border_style); }

        // Yatay Kenarlar
        var hx = left + 1;
        while (hx < right) : (hx += 1) {
            if (buf.getMut(hx, top)) |c| { c.setSymbol("─", 1); c.setStyle(border_style); }
            if (buf.getMut(hx, bottom)) |c| { c.setSymbol("─", 1); c.setStyle(border_style); }
        }

        // Dikey Kenarlar
        var vy = top + 1;
        while (vy < bottom) : (vy += 1) {
            if (buf.getMut(left, vy)) |c| { c.setSymbol("│", 1); c.setStyle(border_style); }
            if (buf.getMut(right, vy)) |c| { c.setSymbol("│", 1); c.setStyle(border_style); }
        }

        // 4. Başlık
        if (self.config.title.len > 0) {
            var title_buf: [128]u8 = undefined;
            const formatted_title = std.fmt.bufPrint(&title_buf, " {s} ", .{self.config.title}) catch self.config.title;
            _ = buf.setString(left + 2, top, formatted_title, self.config.title_style, self.area.width - 4);
        }

        // İçerik Alanı
        return self.area.inner(.{ .horizontal = 2, .vertical = 1 });
    }
};

test "modal calculate area and render frame" {
    const screen = Rect.init(0, 0, 100, 30);
    var modal = Modal.init(.{ .title = "Model Selector", .width_pct = 50, .height_pct = 50 });

    const modal_area = modal.calculateArea(screen);
    try std.testing.expectEqual(@as(u16, 50), modal_area.width);
    try std.testing.expectEqual(@as(u16, 15), modal_area.height);
    try std.testing.expectEqual(@as(u16, 25), modal_area.x);
    try std.testing.expectEqual(@as(u16, 7), modal_area.y);

    var buf = try Buffer.init(std.testing.allocator, screen);
    defer buf.deinit();

    const inner = modal.renderFrame(screen, &buf);
    try std.testing.expect(inner.width > 0 and inner.height > 0);

    // Başlık ekranda bulunmalı
    try std.testing.expect(buf.containsText("Model Selector"));
}
