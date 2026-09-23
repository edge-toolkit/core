//! The build every Zig ws-module runs, and the npm name each is served and published under.
//!
//! Imported as a path dependency rather than by relative path, because Zig refuses an `@import` that escapes
//! the importing module's own directory. Carries no build of its own: the `build` function exists so this is
//! a valid dependency, and does nothing.
//!
//! Not a module directory itself. It holds no `pkg/` and no `package.json`, which is what the hub's scan
//! requires of anything under `services/ws-modules/`, so the scan passes over it rather than serving it.

const std = @import("std");

pub fn build(b: *std.Build) void {
    _ = b;
}

/// The scoped npm package name for a module whose `build.zig.zon` name is `tag`.
///
/// Two things happen to the zon name. Underscores become dashes, because a zon name is a Zig identifier and
/// so cannot hold a dash, while the npm name is expected to. Then the repository owner's scope goes on the
/// front, which is the only shape the registry these are published to accepts -- it rejects an unscoped
/// package outright. The hub drops that scope again when it names a module, so what is served, what a
/// scenario names, and what a deployment refers to are all unchanged by carrying it.
pub fn scopedNpmName(comptime tag: []const u8) []const u8 {
    // A `comptime` block that yields the name, rather than one that returns it: this is called from
    // `addWasmModule`, which is an ordinary runtime function, and a `return` from inside a `comptime` block
    // there fails with "function called at runtime cannot return value at comptime".
    return comptime blk: {
        var buf: [tag.len]u8 = tag[0..tag.len].*;
        for (&buf) |*c| if (c.* == '_') {
            c.* = '-';
        };
        break :blk "@edge-toolkit/" ++ buf;
    };
}

/// What a module's own build script needs back in order to add its extra sources and imports.
pub const WasmModule = struct {
    root_module: *std.Build.Module,
    target: std.Build.ResolvedTarget,
    optimize: std.builtin.OptimizeMode,
};

/// Per-module departures from the standard freestanding-wasm build.
pub const Options = struct {
    /// CPU features to add on top of zig's wasm baseline.
    ///
    /// Needed because `-mcpu baseline` is appended after any per-file cflags, so a feature asked for as a
    /// cflag on one C++ file is stripped again -- it has to be part of the resolved target instead.
    cpu_features_add: std.Target.Cpu.Feature.Set = std.Target.Cpu.Feature.Set.empty,
};

/// Build `src/main.zig` as a freestanding wasm binary and write the `pkg/` the hub serves.
///
/// Installs both halves of what a module directory is: the wasm binary, and the `package.json` naming it.
/// The returned module is still open for a caller to add C sources or imports to -- the install step reads
/// the executable's emitted binary lazily, so adding to it afterwards is what the two modules that need
/// extra sources already do.
pub fn addWasmModule(b: *std.Build, comptime zon: anytype, options: Options) WasmModule {
    const name = @tagName(zon.name);
    const target = b.resolveTargetQuery(.{
        .cpu_arch = .wasm32,
        .cpu_features_add = options.cpu_features_add,
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

    const install = b.addInstallFile(lib.getEmittedBin(), "../pkg/" ++ name ++ ".wasm");
    b.getInstallStep().dependOn(&install.step);

    const pkg_json = std.json.Stringify.valueAlloc(b.allocator, .{
        .name = scopedNpmName(name),
        .type = "module",
        .description = zon.description,
        .version = zon.version,
        .license = zon.license,
        .main = zon.main,
    }, .{ .whitespace = .indent_2 }) catch unreachable;
    const wf = b.addWriteFile("package.json", pkg_json);
    const install_pkg_json = b.addInstallFile(wf.getDirectory().path(b, "package.json"), "../pkg/package.json");
    b.getInstallStep().dependOn(&install_pkg_json.step);

    return .{ .root_module = root_module, .target = target, .optimize = optimize };
}
