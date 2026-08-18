//! omnitrix-net (tasarım Bölüm 3.3).
//!
//! Provider hata taksonomisi (401/403 auth, 429 rate limit, 402 balance,
//! transport, timeout, server 5xx), HTTP request/response formatı ve bounded parser,
//! Server-Sent Events (SSE) akış ayrıştırıcı, bounded retry politikası.
//!
//! Upstream ve leaf modüller için standart ağ ve provider sözleşmelerini sunar.

const std = @import("std");

pub const error_taxonomy = @import("error_taxonomy.zig");
pub const http = @import("http.zig");
pub const sse = @import("sse.zig");
pub const retry = @import("retry.zig");

pub const ProviderErrorKind = error_taxonomy.ProviderErrorKind;

pub const HttpMethod = http.HttpMethod;
pub const HttpHeader = http.HttpHeader;
pub const HttpRequest = http.HttpRequest;
pub const HttpResponse = http.HttpResponse;
pub const HttpParser = http.HttpParser;
pub const HttpParserError = http.HttpParserError;

pub const SseEvent = sse.SseEvent;
pub const SseParser = sse.SseParser;
pub const SseError = sse.SseError;

pub const RetryPolicy = retry.RetryPolicy;

test {
    std.testing.refAllDecls(@This());
}
