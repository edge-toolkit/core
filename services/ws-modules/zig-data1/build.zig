const std = @import("std");
const zon = @import("build.zig.zon");

const npm_name = blk: {
    const s = @tagName(zon.name);
    var buf: [s.len]u8 = s[0..s.len].*;
    for (&buf) |*c| if (c.* == '_') {
        c.* = '-';
    };
    break :blk buf;
};

const name = @tagName(zon.name);
const wasm_install_path = "../pkg/" ++ name ++ ".wasm";

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
        .name = name,
        .root_module = root_module,
    });
    lib.entry = .disabled;
    lib.rdynamic = true;
    root_module.addCSourceFile(.{ .file = b.path("src/util.c") });

    const install = b.addInstallFile(lib.getEmittedBin(), wasm_install_path);
    b.getInstallStep().dependOn(&install.step);

    const pkg_json = std.json.Stringify.valueAlloc(b.allocator, .{
        .name = &npm_name,
        .type = "module",
        .description = zon.description,
        .version = zon.version,
        .license = zon.license,
        .main = zon.main,
    }, .{ .whitespace = .indent_2 }) catch unreachable;
    const wf = b.addWriteFile("package.json", pkg_json);
    const install_pkg_json = b.addInstallFile(wf.getDirectory().path(b, "package.json"), "../pkg/package.json");
    b.getInstallStep().dependOn(&install_pkg_json.step);
}
