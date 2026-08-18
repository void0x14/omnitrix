//! omnitrix-tui: View - OpenCode 1.18.18 Sağ Sidebar Paneli (2D Buffer Widget)
//!
//! Özellikler:
//! - Oturum Başlığı: "Omnitrix projesi ilk adım"
//! - Context: Token sayısı, kullanım yüzdesi, harcanan tutar
//! - ▼ MCP: 20 MCP sunucusunun durum noktaları (yeşil •, gri •, kırmızı •)
//! - LSP: Durum satırı
//! - Alt Bilgi: "~/Belgeler/omnitrix:masterplan" ve "• OpenCode 1.18.18"

const std = @import("std");
const cell_mod = @import("../core/cell.zig");
const geom_mod = @import("../core/geometry.zig");
const buffer_mod = @import("../core/buffer.zig");

pub const Style = cell_mod.Style;
pub const Color = cell_mod.Color;
pub const Rect = geom_mod.Rect;
pub const Buffer = buffer_mod.Buffer;

pub const McpStatus = enum {
    connected,
    disabled,
    err,
};

pub const McpEntry = struct {
    name: []const u8,
    status: McpStatus,
    error_msg: ?[]const u8 = null,
};

pub const SidebarView = struct {
    allocator: std.mem.Allocator,
    session_title: []const u8 = "Omnitrix projesi ilk adım",
    token_count: usize = 688110,
    token_percent: u8 = 66,
    spent_usd: f32 = 1.17,
    branch_info: []const u8 = "~/Belgeler/omnitrix:masterplan",
    version_str: []const u8 = "• OpenCode 1.18.18",

    mcp_servers: std.ArrayList(McpEntry),

    pub fn init(allocator: std.mem.Allocator) SidebarView {
        var self = SidebarView{
            .allocator = allocator,
            .session_title = "Omnitrix projesi ilk adım",
            .token_count = 688110,
            .token_percent = 66,
            .spent_usd = 1.17,
            .branch_info = "~/Belgeler/omnitrix:masterplan",
            .version_str = "• OpenCode 1.18.18",
            .mcp_servers = std.ArrayList(McpEntry).empty,
        };

        // Gerçek MCP Sunucu Listesi (1:1 Ekran Görüntüsü)
        self.addMcp("chrome-devtools", .disabled, null);
        self.addMcp("codebase-memory-mcp", .connected, null);
        self.addMcp("context7", .connected, null);
        self.addMcp("ddgs", .err, "Executable not found in $PATH: \"ddgs\"");
        self.addMcp("exa", .connected, null);
        self.addMcp("ffs", .connected, null);
        self.addMcp("firecrawl", .connected, null);
        self.addMcp("freecad", .err, "MCP error -32000: Connection closed");
        self.addMcp("ghidra", .disabled, null);
        self.addMcp("hashfile", .connected, null);
        self.addMcp("kahin", .connected, null);
        self.addMcp("paper-search-mcp", .connected, null);
        self.addMcp("playwright", .disabled, null);
        self.addMcp("rea", .connected, null);
        self.addMcp("searxng", .connected, null);
        self.addMcp("semble", .connected, null);
        self.addMcp("sequential-thinking", .connected, null);
        self.addMcp("skill-seekers", .disabled, null);
        self.addMcp("tavily", .connected, null);
        self.addMcp("zig-upstream", .disabled, null);

        return self;
    }

    pub fn deinit(self: *SidebarView) void {
        self.mcp_servers.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addMcp(self: *SidebarView, name: []const u8, status: McpStatus, err_msg: ?[]const u8) void {
        self.mcp_servers.append(self.allocator, .{
            .name = name,
            .status = status,
            .error_msg = err_msg,
        }) catch {};
    }

    pub fn render(self: *const SidebarView, area: Rect, buf: *Buffer) void {
        if (area.isEmpty()) return;

        var y = area.top();
        const left = area.left();
        const max_w = area.width;

        // 1. Oturum Başlığı
        _ = buf.setString(left, y, self.session_title, .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w);
        y += 2;

        // 2. Context Bölümü
        if (y < area.bottom()) {
            _ = buf.setString(left, y, "Context", .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w);
            y += 1;
        }

        if (y < area.bottom()) {
            var tok_buf: [64]u8 = undefined;
            const tok_str = std.fmt.bufPrint(&tok_buf, "{d},{d:0>3} tokens", .{ self.token_count / 1000, self.token_count % 1000 }) catch "688,110 tokens";
            _ = buf.setString(left, y, tok_str, .{ .fg = .{ .indexed = 248 } }, max_w);
            y += 1;
        }

        if (y < area.bottom()) {
            var pct_buf: [64]u8 = undefined;
            const pct_str = std.fmt.bufPrint(&pct_buf, "{d}% used", .{self.token_percent}) catch "66% used";
            _ = buf.setString(left, y, pct_str, .{ .fg = .{ .indexed = 244 } }, max_w);
            y += 1;
        }

        if (y < area.bottom()) {
            var spent_buf: [64]u8 = undefined;
            const spent_str = std.fmt.bufPrint(&spent_buf, "${d:.2} spent", .{self.spent_usd}) catch "$1.17 spent";
            _ = buf.setString(left, y, spent_str, .{ .fg = .{ .indexed = 244 } }, max_w);
            y += 2;
        }

        // 3. ▼ MCP Bölümü
        if (y < area.bottom()) {
            _ = buf.setString(left, y, "▼ MCP", .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w);
            y += 1;
        }

        for (self.mcp_servers.items) |mcp| {
            if (y >= area.bottom() - 4) break; // Alt bilgiler için yer bırak

            var cur_x = left;
            switch (mcp.status) {
                .connected => {
                    cur_x += buf.setString(cur_x, y, "• ", .{ .fg = .{ .indexed = 114 } }, max_w);
                    cur_x += buf.setString(cur_x, y, mcp.name, .{ .fg = .bright_white }, max_w - (cur_x - left));
                    _ = buf.setString(cur_x, y, " Connected", .{ .fg = .{ .indexed = 245 } }, max_w - (cur_x - left));
                },
                .disabled => {
                    cur_x += buf.setString(cur_x, y, "• ", .{ .fg = .{ .indexed = 240 } }, max_w);
                    cur_x += buf.setString(cur_x, y, mcp.name, .{ .fg = .{ .indexed = 244 } }, max_w - (cur_x - left));
                    _ = buf.setString(cur_x, y, " Disabled", .{ .fg = .{ .indexed = 240 } }, max_w - (cur_x - left));
                },
                .err => {
                    cur_x += buf.setString(cur_x, y, "• ", .{ .fg = .{ .indexed = 203 } }, max_w);
                    cur_x += buf.setString(cur_x, y, mcp.name, .{ .fg = .bright_white }, max_w - (cur_x - left));
                    const err_str = if (mcp.error_msg) |em| em else "Error";
                    _ = buf.setString(cur_x, y, " ", .{ .fg = .{ .indexed = 242 } }, max_w - (cur_x - left));
                    cur_x += 1;
                    _ = buf.setString(cur_x, y, err_str, .{ .fg = .{ .indexed = 242 }, .modifier = .{ .italic = true } }, max_w - (cur_x - left));
                },
            }
            y += 1;
        }

        // 4. LSP Bölümü
        y = @max(y + 1, area.bottom() - 4);
        if (y < area.bottom() - 2) {
            _ = buf.setString(left, y, "LSP", .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w);
            y += 1;
            _ = buf.setString(left, y, "LSPs will activate as files are read", .{ .fg = .{ .indexed = 242 } }, max_w);
            y += 1;
        }

        // 5. Alt Bilgi
        if (y < area.bottom()) {
            _ = buf.setString(left, y, self.branch_info, .{ .fg = .{ .indexed = 244 } }, max_w);
            y += 1;
        }
        if (y < area.bottom()) {
            var cur_x = left;
            cur_x += buf.setString(cur_x, y, "• ", .{ .fg = .{ .indexed = 114 } }, max_w);
            _ = buf.setString(cur_x, y, "OpenCode 1.18.18", .{ .fg = .bright_white, .modifier = .{ .bold = true } }, max_w - (cur_x - left));
        }
    }
};

test "sidebar view render" {
    const area = Rect.init(0, 0, 40, 40);
    var buf = try Buffer.init(std.testing.allocator, area);
    defer buf.deinit();

    var sb = SidebarView.init(std.testing.allocator);
    defer sb.deinit();

    sb.render(area, &buf);

    const c0 = buf.get(0, 0).?;
    try std.testing.expectEqualStrings("O", c0.getSymbol());
}
