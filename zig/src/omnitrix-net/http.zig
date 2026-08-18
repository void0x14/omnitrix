//! HTTP İstek/Yanıt Veri Yapıları ve Bounded Parser (tasarım Bölüm 3.3, 8).
//!
//! - HTTP/1.1 istek serileştirici (builder)
//! - HTTP/1.1 yanıt ayrıştırıcı (incremental bounded parser)
//! - Bounded header ve body boyut sınırları (sınırsız buffer tutulmaz)
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");

pub const HttpMethod = enum {
    GET,
    POST,
    PUT,
    DELETE,
    HEAD,
    OPTIONS,
    PATCH,

    pub fn asString(self: HttpMethod) []const u8 {
        return @tagName(self);
    }
};

pub const HttpHeader = struct {
    name: []const u8,
    value: []const u8,
};

pub const HttpRequest = struct {
    method: HttpMethod = .GET,
    path: []const u8 = "/",
    host: []const u8 = "",
    headers: []const HttpHeader = &.{},
    body: ?[]const u8 = null,

    /// HTTP/1.1 wire formatını hedef tampona yazar.
    pub fn serialize(self: HttpRequest, out: []u8) !usize {
        var offset: usize = 0;

        const line = try std.fmt.bufPrint(out[offset..], "{s} {s} HTTP/1.1\r\n", .{ self.method.asString(), self.path });
        offset += line.len;

        if (self.host.len > 0) {
            const host_line = try std.fmt.bufPrint(out[offset..], "Host: {s}\r\n", .{self.host});
            offset += host_line.len;
        }

        for (self.headers) |h| {
            const h_line = try std.fmt.bufPrint(out[offset..], "{s}: {s}\r\n", .{ h.name, h.value });
            offset += h_line.len;
        }

        if (self.body) |b| {
            if (!self.hasHeader("Content-Length")) {
                const cl_line = try std.fmt.bufPrint(out[offset..], "Content-Length: {d}\r\n", .{b.len});
                offset += cl_line.len;
            }
            if (offset + 2 + b.len > out.len) return error.NoSpaceLeft;
            @memcpy(out[offset .. offset + 2], "\r\n");
            offset += 2;
            @memcpy(out[offset .. offset + b.len], b);
            offset += b.len;
        } else {
            if (offset + 2 > out.len) return error.NoSpaceLeft;
            @memcpy(out[offset .. offset + 2], "\r\n");
            offset += 2;
        }

        return offset;
    }

    fn hasHeader(self: HttpRequest, name: []const u8) bool {
        for (self.headers) |h| {
            if (std.ascii.eqlIgnoreCase(h.name, name)) return true;
        }
        return false;
    }
};

pub const HttpResponse = struct {
    status_code: u16 = 0,
    status_text: []const u8 = "",
    headers: []const HttpHeader = &.{},
    body: []const u8 = "",

    pub fn getHeader(self: HttpResponse, name: []const u8) ?[]const u8 {
        for (self.headers) |h| {
            if (std.ascii.eqlIgnoreCase(h.name, name)) return h.value;
        }
        return null;
    }
};

pub const HttpParserError = error{
    InvalidStatusLine,
    InvalidHeader,
    HeaderTooLarge,
    BodyTooLarge,
    IncompleteMessage,
    InvalidContentLength,
};

pub const HttpParser = struct {
    allocator: std.mem.Allocator,
    max_header_bytes: usize,
    max_body_bytes: usize,

    pub fn init(allocator: std.mem.Allocator, max_header_bytes: usize, max_body_bytes: usize) HttpParser {
        return .{
            .allocator = allocator,
            .max_header_bytes = max_header_bytes,
            .max_body_bytes = max_body_bytes,
        };
    }

    /// Tampon içerisindeki ham HTTP yanıtını ayrıştırır.
    /// parsed_headers_out dizisine başlıkları yerleştirir.
    pub fn parseResponse(
        self: *const HttpParser,
        raw_data: []const u8,
        headers_out: []HttpHeader,
    ) HttpParserError!HttpResponse {
        // 1. Header sonunu bul (\r\n\r\n veya \n\n)
        const header_end_pos = findHeaderEnd(raw_data) orelse {
            if (raw_data.len > self.max_header_bytes) return error.HeaderTooLarge;
            return error.IncompleteMessage;
        };

        if (header_end_pos > self.max_header_bytes) {
            return error.HeaderTooLarge;
        }

        const header_part = raw_data[0..header_end_pos];
        const body_part = raw_data[header_end_pos + 4 ..];

        // 2. Status line ayrıştır
        var line_it = std.mem.splitSequence(u8, header_part, "\r\n");
        const status_line = line_it.first();

        var status_tokens = std.mem.tokenizeScalar(u8, status_line, ' ');
        const http_ver = status_tokens.next() orelse return error.InvalidStatusLine;
        if (!std.mem.startsWith(u8, http_ver, "HTTP/")) return error.InvalidStatusLine;

        const code_str = status_tokens.next() orelse return error.InvalidStatusLine;
        const code = std.fmt.parseInt(u16, code_str, 10) catch return error.InvalidStatusLine;

        const status_text = status_tokens.rest();

        // 3. Başlıkları ayrıştır
        var header_count: usize = 0;
        while (line_it.next()) |line| {
            if (line.len == 0) continue;
            const colon_idx = std.mem.indexOfScalar(u8, line, ':') orelse return error.InvalidHeader;
            const name = std.mem.trim(u8, line[0..colon_idx], " \t");
            const val = std.mem.trim(u8, line[colon_idx + 1 ..], " \t");

            if (header_count < headers_out.len) {
                headers_out[header_count] = .{ .name = name, .value = val };
                header_count += 1;
            }
        }

        // 4. Content-Length kontrolü
        var expected_content_len: ?usize = null;
        for (headers_out[0..header_count]) |h| {
            if (std.ascii.eqlIgnoreCase(h.name, "Content-Length")) {
                const len = std.fmt.parseInt(usize, h.value, 10) catch return error.InvalidContentLength;
                expected_content_len = len;
                break;
            }
        }

        if (expected_content_len) |exp_len| {
            if (exp_len > self.max_body_bytes) return error.BodyTooLarge;
            if (body_part.len < exp_len) return error.IncompleteMessage;
        }

        if (body_part.len > self.max_body_bytes) return error.BodyTooLarge;

        return .{
            .status_code = code,
            .status_text = status_text,
            .headers = headers_out[0..header_count],
            .body = body_part,
        };
    }

    fn findHeaderEnd(data: []const u8) ?usize {
        return std.mem.indexOf(u8, data, "\r\n\r\n");
    }
};

test "http request serialize" {
    const headers = [_]HttpHeader{
        .{ .name = "Authorization", .value = "Bearer sk-test" },
        .{ .name = "Content-Type", .value = "application/json" },
    };
    const req = HttpRequest{
        .method = .POST,
        .path = "/v1/chat/completions",
        .host = "api.openai.com",
        .headers = &headers,
        .body = "{\"model\": \"gpt-4o\"}",
    };

    var buf: [512]u8 = undefined;
    const len = try req.serialize(&buf);
    const wire = buf[0..len];

    try std.testing.expect(std.mem.indexOf(u8, wire, "POST /v1/chat/completions HTTP/1.1\r\n") != null);
    try std.testing.expect(std.mem.indexOf(u8, wire, "Host: api.openai.com\r\n") != null);
    try std.testing.expect(std.mem.indexOf(u8, wire, "Authorization: Bearer sk-test\r\n") != null);
    try std.testing.expect(std.mem.indexOf(u8, wire, "Content-Length: 19\r\n") != null);
    try std.testing.expect(std.mem.indexOf(u8, wire, "{\"model\": \"gpt-4o\"}") != null);
}

test "http response parse" {
    const raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"ok\": true}\n";
    const parser = HttpParser.init(std.testing.allocator, 1024, 4096);

    var headers_buf: [8]HttpHeader = undefined;
    const resp = try parser.parseResponse(raw, &headers_buf);

    try std.testing.expectEqual(@as(u16, 200), resp.status_code);
    try std.testing.expectEqualStrings("OK", resp.status_text);
    try std.testing.expectEqualStrings("application/json", resp.getHeader("Content-Type").?);
    try std.testing.expectEqualStrings("{\"ok\": true}\n", resp.body);
}

test "http response header limit asimi" {
    const raw = "HTTP/1.1 200 OK\r\nX-Very-Long: 12345678901234567890\r\n\r\n";
    const parser = HttpParser.init(std.testing.allocator, 10, 4096); // 10 byte limit

    var headers_buf: [4]HttpHeader = undefined;
    try std.testing.expectError(error.HeaderTooLarge, parser.parseResponse(raw, &headers_buf));
}
