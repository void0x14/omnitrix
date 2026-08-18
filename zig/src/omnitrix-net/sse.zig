//! Server-Sent Events (SSE) Streaming Parser (tasarım Bölüm 3.3, 8).
//!
//! - Bounded buffer ile parçalı (chunked/streaming) SSE ayrıştırma
//! - LLM akışlarında yaygın olan `data: [DONE]` desteği
//! - Multi-line `data:` alanlarını birleştirme
//! - Sınırsız tampon tutulmaz: satır ve olay boyutu bounded limitlerle korunur
//! - Üretim yolunda `catch unreachable` veya `@panic` YOKTUR.

const std = @import("std");

pub const SseEvent = struct {
    event_type: ?[]const u8 = null,
    data: []const u8 = "",
    id: ?[]const u8 = null,
    retry_ms: ?u64 = null,
    is_done: bool = false,
};

pub const SseError = error{
    LineTooLong,
    EventTooLarge,
    BufferFull,
    InvalidRetryValue,
};

pub const SseParser = struct {
    allocator: std.mem.Allocator,
    max_line_bytes: usize,
    max_event_bytes: usize,

    // Gelen ham akış için biriktirici
    buffer: std.ArrayList(u8),
    // Mevcut event için toplanan veri
    accumulating_data: std.ArrayList(u8),
    // Dispatch edilen son event verisi (bir sonraki nextEvent'e kadar geçerli)
    dispatched_data: std.ArrayList(u8),

    event_type_buf: [128]u8 = undefined,
    event_type_len: ?usize = null,

    id_buf: [128]u8 = undefined,
    id_len: ?usize = null,

    current_retry_ms: ?u64 = null,

    pub fn init(allocator: std.mem.Allocator, max_line_bytes: usize, max_event_bytes: usize) SseParser {
        return .{
            .allocator = allocator,
            .max_line_bytes = max_line_bytes,
            .max_event_bytes = max_event_bytes,
            .buffer = std.ArrayList(u8).empty,
            .accumulating_data = std.ArrayList(u8).empty,
            .dispatched_data = std.ArrayList(u8).empty,
            .event_type_len = null,
            .id_len = null,
            .current_retry_ms = null,
        };
    }

    pub fn deinit(self: *SseParser) void {
        self.buffer.deinit(self.allocator);
        self.accumulating_data.deinit(self.allocator);
        self.dispatched_data.deinit(self.allocator);
        self.* = undefined;
    }

    /// Yeni gelen ağ parçasını tampona ekler.
    pub fn feed(self: *SseParser, chunk: []const u8) SseError!void {
        if (self.buffer.items.len + chunk.len > self.max_line_bytes * 4) {
            return error.BufferFull;
        }
        self.buffer.appendSlice(self.allocator, chunk) catch return error.BufferFull;
    }

    /// Sıradaki tam SSE olayını döner. Hazır olay yoksa null döner.
    pub fn nextEvent(self: *SseParser) SseError!?SseEvent {
        while (true) {
            // Tam bir satır sonu ara (\n)
            const newline_pos = std.mem.indexOfScalar(u8, self.buffer.items, '\n') orelse {
                if (self.buffer.items.len > self.max_line_bytes) {
                    return error.LineTooLong;
                }
                return null;
            };

            var line = self.buffer.items[0..newline_pos];
            if (line.len > 0 and line[line.len - 1] == '\r') {
                line = line[0 .. line.len - 1];
            }

            if (line.len > self.max_line_bytes) {
                return error.LineTooLong;
            }

            // Satırı ayrıştır (tüketmeden önce)
            const is_empty_line = (line.len == 0);
            const is_comment = (line.len > 0 and line[0] == ':');

            if (!is_empty_line and !is_comment) {
                if (std.mem.indexOfScalar(u8, line, ':')) |colon_idx| {
                    const field_name = line[0..colon_idx];
                    var field_val = line[colon_idx + 1 ..];
                    if (field_val.len > 0 and field_val[0] == ' ') {
                        field_val = field_val[1..];
                    }

                    if (std.mem.eql(u8, field_name, "data")) {
                        if (self.accumulating_data.items.len + field_val.len + 1 > self.max_event_bytes) {
                            return error.EventTooLarge;
                        }
                        if (self.accumulating_data.items.len > 0) {
                            self.accumulating_data.append(self.allocator, '\n') catch return error.EventTooLarge;
                        }
                        self.accumulating_data.appendSlice(self.allocator, field_val) catch return error.EventTooLarge;
                    } else if (std.mem.eql(u8, field_name, "event")) {
                        const copy_len = @min(field_val.len, self.event_type_buf.len);
                        @memcpy(self.event_type_buf[0..copy_len], field_val[0..copy_len]);
                        self.event_type_len = copy_len;
                    } else if (std.mem.eql(u8, field_name, "id")) {
                        const copy_len = @min(field_val.len, self.id_buf.len);
                        @memcpy(self.id_buf[0..copy_len], field_val[0..copy_len]);
                        self.id_len = copy_len;
                    } else if (std.mem.eql(u8, field_name, "retry")) {
                        self.current_retry_ms = std.fmt.parseInt(u64, field_val, 10) catch return error.InvalidRetryValue;
                    }
                }
            }

            // Tüketilen satırı ana tampondan çıkar
            const consume_len = newline_pos + 1;
            const remaining_len = self.buffer.items.len - consume_len;
            std.mem.copyForwards(u8, self.buffer.items[0..remaining_len], self.buffer.items[consume_len..]);
            self.buffer.items.len = remaining_len;

            // Boş satır -> olayı yayınla / dispatch et
            if (is_empty_line) {
                if (self.accumulating_data.items.len > 0 or self.event_type_len != null) {
                    // Veriyi dispatched_data'ya aktar
                    self.dispatched_data.clearRetainingCapacity();
                    self.dispatched_data.appendSlice(self.allocator, self.accumulating_data.items) catch return error.EventTooLarge;
                    self.accumulating_data.clearRetainingCapacity();

                    const data_slice = self.dispatched_data.items;
                    const is_done = std.mem.eql(u8, data_slice, "[DONE]");

                    const ev_type: ?[]const u8 = if (self.event_type_len) |len| self.event_type_buf[0..len] else null;
                    const ev_id: ?[]const u8 = if (self.id_len) |len| self.id_buf[0..len] else null;

                    const ev = SseEvent{
                        .event_type = ev_type,
                        .data = data_slice,
                        .id = ev_id,
                        .retry_ms = self.current_retry_ms,
                        .is_done = is_done,
                    };

                    self.event_type_len = null;
                    self.id_len = null;
                    self.current_retry_ms = null;

                    return ev;
                }
            }
        }
    }
};

test "sse standart veri ve [DONE] ayristirma" {
    var parser = SseParser.init(std.testing.allocator, 1024, 4096);
    defer parser.deinit();

    const stream_data =
        "event: message\r\n" ++
        "data: {\"choices\": [{\"delta\": {\"content\": \"hello\"}}]}\r\n\r\n" ++
        "data: [DONE]\r\n\r\n";

    try parser.feed(stream_data);

    const ev1 = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("message", ev1.event_type.?);
    try std.testing.expectEqualStrings("{\"choices\": [{\"delta\": {\"content\": \"hello\"}}]}", ev1.data);
    try std.testing.expect(!ev1.is_done);

    const ev2 = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("[DONE]", ev2.data);
    try std.testing.expect(ev2.is_done);

    const ev3 = try parser.nextEvent();
    try std.testing.expect(ev3 == null);
}

test "sse parcali (chunked) akis ayristirma" {
    var parser = SseParser.init(std.testing.allocator, 1024, 4096);
    defer parser.deinit();

    // Parça 1: yarım satır
    try parser.feed("data: hel");
    try std.testing.expect((try parser.nextEvent()) == null);

    // Parça 2: satırın kalanı ve sonlandırıcı
    try parser.feed("lo world\n\n");
    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("hello world", ev.data);
}

test "sse multi-line data birlestirme" {
    var parser = SseParser.init(std.testing.allocator, 1024, 4096);
    defer parser.deinit();

    const stream_data =
        "data: line 1\n" ++
        "data: line 2\n\n";

    try parser.feed(stream_data);
    const ev = (try parser.nextEvent()).?;
    try std.testing.expectEqualStrings("line 1\nline 2", ev.data);
}
