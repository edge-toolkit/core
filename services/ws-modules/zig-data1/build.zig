const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.resolveTargetQuery(.{
        .cpu_arch = .wasm32,
        .os_tag = .freestanding,
    });
    const optimize = b.standardOptimizeOption(.{});

    const root_module = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });

    const lib = b.addExecutable(.{
        .name = "et_ws_zig_data1",
        .root_module = root_module,
    });
    lib.entry = .disabled;
    lib.rdynamic = true;

    const install = b.addInstallFile(lib.getEmittedBin(), "../pkg/et_ws_zig_data1.wasm");
    b.getInstallStep().dependOn(&install.step);
}
