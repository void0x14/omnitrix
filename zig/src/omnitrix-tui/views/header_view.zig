//! omnitrix-tui: View - Header Tabs (Üst OS Sekmeler Barı)
//!
//! Özellikler:
//! - 1:1 OpenCode 1.18.18 Üst Sekme Düzeni
//! - Sekmeler:
//!   - Tab 1: "  OC | Omnitrix projesi ilk adım ✕  "
//!   - Tab 2: "  agy --dangerously-skip-permissions  "
//!   - Tab 3: "  .../Belgeler/omnitrix/zig  "
//! - Aktif sekme: Parlak beyaz ve koyu gri arka plan
//! - Pasif sekme: Koyu gri yazı ve siyahımsı arka plan

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");

pub const Style = cell_mod.Style;
pub const Color = cell_mod.Color;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;

pub const HeaderView = struct {
    active_tab: usize = 0,

    pub fn init() HeaderView {
        return .{ .active_tab = 0 };
    }

    pub fn render(self: *const HeaderView, area: Rect, buf: *Buffer) void {
        if (area.isEmpty()) return;

        const bg_bar = Style{ .bg = .{ .indexed = 234 } };
        buf.setStyle(area, bg_bar);

        var x = area.left();
        const y = area.top();

        const tabs = [_][]const u8{
            "  OC | Omnitrix projesi ilk adım ✕  ",
            "  agy --dangerously-skip-permissions  ",
            "  .../Belgeler/omnitrix/zig  ",
        };

        const active_style = Style{
            .fg = .bright_white,
            .bg = .{ .indexed = 237 },
            .modifier = .{ .bold = true },
        };

        const inactive_style = Style{
            .fg = .{ .indexed = 244 },
            .bg = .{ .indexed = 234 },
        };

        for (tabs, 0..) |tab_text, idx| {
            if (x >= area.right()) break;
            const st = if (idx == self.active_tab) active_style else inactive_style;
            const w = buf.setString(x, y, tab_text, st, area.right() - x);
            x += w;
        }
    }
};

test "header view render" {
    const area = Rect.init(0, 0, 100, 1);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    const hv = HeaderView.init();
    hv.render(area, &buf);

    const c = buf.get(2, 0).?;
    try std.testing.expectEqualStrings("O", c.getSymbol());
}
