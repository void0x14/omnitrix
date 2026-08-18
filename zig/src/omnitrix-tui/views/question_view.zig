//! omnitrix-tui: View - OpenCode 1.18.18 Question & Choice Modal (2D Buffer Widget)
//!
//! Özellikler:
//! - Mod / Model Rozeti: ▣ Build · MiMo-V2.5-Pro
//! - Sol Dikey Çizgi ('│')
//! - Numaralandırılmış Seçenekler (1..N) ve Açıklamalar
//! - Serbest Yanıt ("Type your own answer")
//! - Alt Kısayollar: ↑↓ select   enter submit   esc dismiss

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");

pub const Style = cell_mod.Style;
pub const Color = cell_mod.Color;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;

pub const QuestionOption = struct {
    title: []const u8,
    description: ?[]const u8 = null,
};

pub const QuestionView = struct {
    allocator: std.mem.Allocator,
    mode_name: []const u8 = "Build",
    model_name: []const u8 = "MiMo-V2.5-Pro",
    question_text: []const u8,
    options: std.ArrayList(QuestionOption),
    selected_idx: usize = 0,
    is_input_mode: bool = false,
    custom_input: std.ArrayList(u8),

    pub fn init(allocator: std.mem.Allocator, question: []const u8) QuestionView {
        var self = QuestionView{
            .allocator = allocator,
            .mode_name = "Build",
            .model_name = "MiMo-V2.5-Pro",
            .question_text = question,
            .options = std.ArrayList(QuestionOption).empty,
            .selected_idx = 0,
            .is_input_mode = false,
            .custom_input = std.ArrayList(u8).empty,
        };

        // Varsayılan seçenekler (Ekran görüntüsü 1:1)
        self.addOption("Crush'dan al, ekle", "Crush'daki fold + iptal + döngü tespitini alıp Omnitrix'e ekleyeceğiz. Senin kodunda olmayan kısımlar crush'dan tamamlanacak.");
        self.addOption("Kendi kodundakileri kullan", "Senin kodundaki mekanizmaları (combine, doom_loop sinyali, goal stall) temel alacağız, crush'dan bir şey eklemeyeceğiz.");
        self.addOption("İkisini birleştir", "İkisinin de en iyi kısımlarını birleştireceğiz: crush'ın kesin iptal mekanizması + senin kodundaki combine ve goal stall.");

        return self;
    }

    pub fn deinit(self: *QuestionView) void {
        self.options.deinit(self.allocator);
        self.custom_input.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addOption(self: *QuestionView, title: []const u8, desc: ?[]const u8) void {
        self.options.append(self.allocator, .{
            .title = title,
            .description = desc,
        }) catch {};
    }

    pub fn handleKey(self: *QuestionView, key: []const u8) !bool {
        const total_rows = self.options.items.len + 1; // +1 for custom answer

        if (!self.is_input_mode) {
            // Yukarı (Up / k)
            if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'A') or (key.len == 1 and key[0] == 'k')) {
                if (self.selected_idx > 0) {
                    self.selected_idx -= 1;
                    return true;
                }
            }
            // Aşağı (Down / j)
            else if ((key.len == 3 and key[0] == '\x1b' and key[1] == '[' and key[2] == 'B') or (key.len == 1 and key[0] == 'j')) {
                if (self.selected_idx + 1 < total_rows) {
                    self.selected_idx += 1;
                    return true;
                }
            }
            // Enter
            else if (key.len == 1 and (key[0] == '\r' or key[0] == '\n')) {
                if (self.selected_idx == self.options.items.len) {
                    self.is_input_mode = true;
                    return true;
                }
            }
        } else {
            // Input mode
            if (key.len == 1 and key[0] == 27) { // Esc
                self.is_input_mode = false;
                return true;
            } else if (key.len == 1 and (key[0] == 127 or key[0] == 8)) { // Backspace
                if (self.custom_input.items.len > 0) {
                    _ = self.custom_input.pop();
                    return true;
                }
            } else if (key.len == 1 and key[0] >= 32 and key[0] <= 126) {
                try self.custom_input.append(self.allocator, key[0]);
                return true;
            }
        }
        return false;
    }

    pub fn render(self: *const QuestionView, area: Rect, buf: *Buffer) void {
        if (area.isEmpty()) return;

        var y = area.top();
        const left = area.left();
        const max_w = area.width;

        // 1. Mod / Model Rozeti: ▣ Build · MiMo-V2.5-Pro
        var cur_x = left;
        cur_x += buf.setString(cur_x, y, "▣ ", .{ .fg = .{ .indexed = 111 } }, max_w);
        cur_x += buf.setString(cur_x, y, self.mode_name, .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w - (cur_x - left));
        cur_x += buf.setString(cur_x, y, " · ", .{ .fg = .{ .indexed = 244 } }, max_w - (cur_x - left));
        _ = buf.setString(cur_x, y, self.model_name, .{ .fg = .{ .indexed = 244 } }, max_w - (cur_x - left));
        y += 2;

        const bar_style = Style{ .fg = .{ .indexed = 111 } }; // Açık mor/mavi sol çizgi

        // 2. Soru Başlığı (│ Soru Metni)
        if (y < area.bottom()) {
            _ = buf.setString(left, y, "│ ", bar_style, max_w);
            _ = buf.setString(left + 2, y, self.question_text, .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w - 2);
            y += 1;
        }

        if (y < area.bottom()) {
            _ = buf.setString(left, y, "│", bar_style, max_w);
            y += 1;
        }

        // 3. Seçenekler Listesi
        for (self.options.items, 0..) |opt, idx| {
            if (y >= area.bottom() - 2) break;

            const is_selected = (self.selected_idx == idx);

            _ = buf.setString(left, y, "│ ", bar_style, max_w);

            var num_buf: [16]u8 = undefined;
            const num_str = std.fmt.bufPrint(&num_buf, "{d}. ", .{idx + 1}) catch "1. ";

            var opt_x = left + 2;
            if (is_selected) {
                opt_x += buf.setString(opt_x, y, num_str, .{ .fg = .{ .indexed = 117 }, .modifier = .{ .bold = true } }, max_w - 2);
                _ = buf.setString(opt_x, y, opt.title, .{ .fg = .{ .indexed = 117 }, .modifier = .{ .bold = true } }, max_w - (opt_x - left));
            } else {
                opt_x += buf.setString(opt_x, y, num_str, .{ .fg = .{ .indexed = 246 } }, max_w - 2);
                _ = buf.setString(opt_x, y, opt.title, .{ .fg = .bright_white }, max_w - (opt_x - left));
            }
            y += 1;

            if (opt.description) |desc| {
                if (y < area.bottom() - 2) {
                    _ = buf.setString(left, y, "│    ", bar_style, max_w);
                    _ = buf.setString(left + 5, y, desc, .{ .fg = .{ .indexed = 244 } }, max_w - 5);
                    y += 1;
                }
            }
        }

        // 4. "Type your own answer"
        if (y < area.bottom() - 2) {
            const is_custom_selected = (self.selected_idx == self.options.items.len);
            var custom_num_buf: [16]u8 = undefined;
            const custom_num_str = std.fmt.bufPrint(&custom_num_buf, "{d}. ", .{self.options.items.len + 1}) catch "4. ";

            _ = buf.setString(left, y, "│ ", bar_style, max_w);
            var c_x = left + 2;

            if (is_custom_selected) {
                c_x += buf.setString(c_x, y, custom_num_str, .{ .fg = .{ .indexed = 117 }, .modifier = .{ .bold = true } }, max_w - 2);
                c_x += buf.setString(c_x, y, "Type your own answer", .{ .fg = .{ .indexed = 117 }, .modifier = .{ .bold = true } }, max_w - (c_x - left));
                if (self.is_input_mode) {
                    c_x += buf.setString(c_x, y, ": ", .{ .fg = .bright_white }, max_w - (c_x - left));
                    c_x += buf.setString(c_x, y, self.custom_input.items, .{ .fg = .bright_white }, max_w - (c_x - left));
                    _ = buf.setString(c_x, y, "▋", .{ .fg = .bright_cyan }, max_w - (c_x - left));
                }
            } else {
                c_x += buf.setString(c_x, y, custom_num_str, .{ .fg = .{ .indexed = 246 } }, max_w - 2);
                _ = buf.setString(c_x, y, "Type your own answer", .{ .fg = .bright_white }, max_w - (c_x - left));
            }
            y += 1;
        }

        // 5. Alt Kısayollar
        if (y < area.bottom()) {
            _ = buf.setString(left, y, "│", bar_style, max_w);
            y += 1;
        }
        if (y < area.bottom()) {
            _ = buf.setString(left + 2, y, "↑↓ select   enter submit   esc dismiss", .{ .fg = .{ .indexed = 244 } }, max_w - 2);
        }
    }
};

test "question view render" {
    const area = Rect.init(0, 0, 80, 20);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var qv = QuestionView.init(std.testing.allocator, "Hangi mekanizma?");
    defer qv.deinit();

    qv.render(area, &buf);

    const c0 = buf.get(0, 0).?;
    try std.testing.expectEqualStrings("▣", c0.getSymbol());
}
