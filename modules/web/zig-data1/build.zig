const std = @import("std");
const zon = @import("build.zig.zon");
const shared = @import("zig_shared");

pub fn build(b: *std.Build) void {
    const module = shared.addWasmModule(b, zon, .{});

    // Generated REST client (path-pinned, lives under generated/zig-rest/).
    // The single `extern fn js_rest_request` it relies on is satisfied by
    // the worker shim in pkg/.
    const rest_module = b.createModule(.{
        .root_source_file = b.path("../../../generated/zig-rest/src/et_rest_client.zig"),
        .target = module.target,
        .optimize = module.optimize,
    });
    module.root_module.addImport("et_rest_client", rest_module);

    module.root_module.addCSourceFile(.{ .file = b.path("src/util.c") });
    // C++ compiles through the same clang, but freestanding wasm32 has no libc++: keep exceptions and RTTI
    // off so nothing references the missing C++ runtime (unwind tables, type_info, __cxa_* symbols).
    module.root_module.addCSourceFile(.{
        .file = b.path("src/util.cpp"),
        .flags = &.{ "-fno-exceptions", "-fno-rtti" },
    });
}
