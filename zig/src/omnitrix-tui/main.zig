const std = @import("std");
const app_mod = @import("app.zig");
const App = app_mod.App;

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .{};
    _ = gpa.deinit();
    const allocator = gpa.allocator();

    var app = try App.init(allocator);
    defer app.deinit();

    app.run() catch |err| {
        app.deinit();
        return err;
    };
}
