const std = @import("std");
const zon = @import("build.zig.zon");
const shared = @import("zig_shared");

pub fn build(b: *std.Build) void {
    // exception_handling joins the target rather than arriving as a cflag on exceptions.cpp, because
    // `-mcpu baseline` is appended after per-file cflags and would strip it again -- its
    // __builtin_wasm_throw then fails with "needs target feature exception-handling". Zig itself emits no EH
    // instructions; only the exception-enabled C++ TU uses the feature.
    const module = shared.addWasmModule(b, zon, .{
        .cpu_features_add = std.Target.wasm.featureSet(&.{.exception_handling}),
    });

    // Exception-enabled C++ TU: real wasm exception-handling instructions plus the minimal runtime defined in
    // the file itself. See src/exceptions.cpp for the model and its catch (...)-only constraint.
    module.root_module.addCSourceFile(.{
        .file = b.path("src/exceptions.cpp"),
        .flags = &.{ "-fwasm-exceptions", "-mexception-handling", "-fno-rtti" },
    });
}
