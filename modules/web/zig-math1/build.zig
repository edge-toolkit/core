const std = @import("std");
const zon = @import("build.zig.zon");
const shared = @import("zig_shared");

pub fn build(b: *std.Build) void {
    _ = shared.addWasmModule(b, zon, .{});
}
