//! omnitrix-tui: OpenCode 1.18.18 Sağ Sidebar Paneli
//!
//! Özellikler (Screenshot 1:1 Birebir):
//! - Başlık: Oturum Adı (örn. "Omnitrix projesi ilk adım")
//! - Context: Token sayısı (örn. "688,110 tokens"), "%66 used", "$1.17 spent"
//! - ▼ MCP: Sunucu listesi ve durum noktaları (yeşil • Connected, gri • Disabled, kırmızı • Error)
//! - LSP: "LSPs will activate as files are read"
//! - Alt Bilgi: "~/Belgeler/omnitrix:masterplan" ve "• OpenCode 1.18.18"

const std = @import("std");
const term = @import("terminal.zig");
const theme = @import("theme.zig");
const unicode = @import("unicode.zig");

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

pub const Sidebar = struct {
    allocator: std.mem.Allocator,
    session_title: []const u8 = "Omnitrix projesi ilk adım",
    token_count: usize = 688110,
    token_percent: u8 = 66,
    spent_usd: f32 = 1.17,
    branch_info: []const u8 = "~/Belgeler/omnitrix:masterplan",
    version_str: []const u8 = "• OpenCode 1.18.18",

    mcp_servers: std.ArrayList(McpEntry),

    pub fn init(allocator: std.mem.Allocator) Sidebar {
        var self = Sidebar{
            .allocator = allocator,
            .session_title = "Omnitrix projesi ilk adım",
            .token_count = 688110,
            .token_percent = 66,
            .spent_usd = 1.17,
            .branch_info = "~/Belgeler/omnitrix:masterplan",
            .version_str = "• OpenCode 1.18.18",
            .mcp_servers = std.ArrayList(McpEntry).empty,
        };

        // Varsayılan MCP listesi (Ekran görüntüsündeki gerçek MCP sunucuları)
        self.addMcp("chrome-devtools", .disabled, null) catch {};
        self.addMcp("codebase-memory-mcp", .connected, null) catch {};
        self.addMcp("context7", .connected, null) catch {};
        self.addMcp("ddgs", .err, "Executable not found in $PATH: \"ddgs\"") catch {};
        self.addMcp("exa", .connected, null) catch {};
        self.addMcp("ffs", .connected, null) catch {};
        self.addMcp("firecrawl", .connected, null) catch {};
        self.addMcp("freecad", .err, "MCP error -32000: Connection closed") catch {};
        self.addMcp("ghidra", .disabled, null) catch {};
        self.addMcp("hashfile", .connected, null) catch {};
        self.addMcp("kahin", .connected, null) catch {};
        self.addMcp("paper-search-mcp", .connected, null) catch {};
        self.addMcp("playwright", .disabled, null) catch {};
        self.addMcp("rea", .connected, null) catch {};
        self.addMcp("searxng", .connected, null) catch {};
        self.addMcp("semble", .connected, null) catch {};
        self.addMcp("sequential-thinking", .connected, null) catch {};
        self.addMcp("skill-seekers", .disabled, null) catch {};
        self.addMcp("tavily", .connected, null) catch {};
        self.addMcp("zig-upstream", .disabled, null) catch {};

        return self;
    }

    pub fn deinit(self: *Sidebar) void {
        self.mcp_servers.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn addMcp(self: *Sidebar, name: []const u8, status: McpStatus, err_msg: ?[]const u8) !void {
        try self.mcp_servers.append(self.allocator, .{
            .name = name,
            .status = status,
            .error_msg = err_msg,
        });
    }

    pub fn renderToLines(self: *Sidebar, allocator: std.mem.Allocator, width: usize) !std.ArrayList([]const u8) {
        var lines = std.ArrayList([]const u8).empty;

        // 1. Başlık
        var l_buf = std.ArrayList(u8).empty;
        defer l_buf.deinit(allocator);

        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, self.session_title);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 2. Context Bölümü
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, "Context");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        var tok_buf: [64]u8 = undefined;
        const tok_str = std.fmt.bufPrint(&tok_buf, "{d},{d:0>3} tokens", .{ self.token_count / 1000, self.token_count % 1000 }) catch "688,110 tokens";
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 248 } });
        try l_buf.appendSlice(allocator, tok_str);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        var pct_buf: [64]u8 = undefined;
        const pct_str = std.fmt.bufPrint(&pct_buf, "{d}% used", .{self.token_percent}) catch "66% used";
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, pct_str);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        var spent_buf: [64]u8 = undefined;
        const spent_str = std.fmt.bufPrint(&spent_buf, "${d:.2} spent", .{self.spent_usd}) catch "$1.17 spent";
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, spent_str);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 3. ▼ MCP Bölümü
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, "▼ MCP");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        for (self.mcp_servers.items) |mcp| {
            l_buf.clearRetainingCapacity();
            switch (mcp.status) {
                .connected => {
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 114 } }); // Pastel yeşil
                    try l_buf.appendSlice(allocator, "• ");
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white });
                    try l_buf.appendSlice(allocator, mcp.name);
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 245 } });
                    try l_buf.appendSlice(allocator, " Connected");
                },
                .disabled => {
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 240 } }); // Koyu gri
                    try l_buf.appendSlice(allocator, "• ");
                    try l_buf.appendSlice(allocator, mcp.name);
                    try l_buf.appendSlice(allocator, " Disabled");
                },
                .err => {
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 203 } }); // Pastel kırmızı
                    try l_buf.appendSlice(allocator, "• ");
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white });
                    try l_buf.appendSlice(allocator, mcp.name);
                    try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 242 }, .italic = true });
                    try l_buf.appendSlice(allocator, " ");
                    if (mcp.error_msg) |em| {
                        try l_buf.appendSlice(allocator, em);
                    } else {
                        try l_buf.appendSlice(allocator, "Error");
                    }
                },
            }
            try l_buf.appendSlice(allocator, theme.ANSI.reset);
            const trunc = try unicode.truncateToWidth(allocator, l_buf.items, width, "..");
            defer allocator.free(trunc);
            try lines.append(allocator, try allocator.dupe(u8, trunc));
        }

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 4. LSP Bölümü
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, "LSP");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 242 } });
        try l_buf.appendSlice(allocator, "LSPs will activate as files are read");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try lines.append(allocator, try allocator.dupe(u8, ""));

        // 5. Alt Bilgi
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 244 } });
        try l_buf.appendSlice(allocator, self.branch_info);
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));
        l_buf.clearRetainingCapacity();

        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color{ .ansi = 114 } }); // Yeşil nokta
        try l_buf.appendSlice(allocator, "• ");
        try term.appendStyle(&l_buf, allocator, .{ .fg = term.Color.bright_white, .bold = true });
        try l_buf.appendSlice(allocator, "OpenCode 1.18.18");
        try l_buf.appendSlice(allocator, theme.ANSI.reset);
        try lines.append(allocator, try allocator.dupe(u8, l_buf.items));

        return lines;
    }
};

test "sidebar render lines" {
    var sb = Sidebar.init(std.testing.allocator);
    defer sb.deinit();

    var lines = try sb.renderToLines(std.testing.allocator, 40);
    defer {
        for (lines.items) |l| std.testing.allocator.free(l);
        lines.deinit(std.testing.allocator);
    }

    try std.testing.expect(lines.items.len > 10);
}
