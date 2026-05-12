const std = @import("std");
const zon = @import("build.zig.zon");

// Per-model flash totals, written by tools/compute_flash.py during `mise run
// codegen`. Each file is `pub const flash_bytes: u32 = N;` where N is the sum
// of every const-qualified array in that model's genModel.{h,c} — the actual
// flash-resident bytes (int8 weights + fp32 weight copies for training +
// biases in both formats + requantization metadata). Replaces the upstream
// `#define MODEL_SIZE` number, which counted a different subset.
const flash_mcunet = @import("codegen-mcunet/flash_bytes.zig").flash_bytes;
const flash_mbv2 = @import("codegen-mbv2/flash_bytes.zig").flash_bytes;
const flash_proxyless = @import("codegen-proxyless/flash_bytes.zig").flash_bytes;

const npm_name = blk: {
    const s = @tagName(zon.name);
    var buf: [s.len]u8 = s[0..s.len].*;
    for (&buf) |*c| if (c.* == '_') {
        c.* = '-';
    };
    break :blk buf;
};

const base_name = @tagName(zon.name);

const ModelCfg = struct {
    name: []const u8,
    codegen_dir: []const u8,
    pooled_offset: u32,
    pooled_count: u32,
    has_train_graph: u1,
    peak_mem: u32,
    model_size: u32,
    // Paper Figure 10 memory comparison (peak SRAM in bytes). Measured by
    // running tinyengine's GeneralMemoryScheduler on each variant of the
    // training graph for this backbone — see the regen pipeline notes in
    // project_mcu_shim_pitfalls. These are static analysis bounds, not
    // runtime measurements.
    //   ft_full_sram = full backward pass (every weight trainable). Run
    //                  on the full_bp-1x3x128x128.ir triplet with the
    //                  FUSE_SGD_UPDATE_STR fusion disabled (otherwise the
    //                  parser hits an unhandled transpose pattern at
    //                  TTEParser.py:438). All other optimizations off.
    //   ft_su_sram   = sparse update with NO codegen optimizations
    //                  (inplace=False, sort_by_lifetime=False). The
    //                  paper's "FT-SU" baseline.
    //   ft_sur_sram  = sparse update + in-place + lifetime sort = peak_mem.
    //                  The paper's "FT-SU+R" (Reorder) — what this wasm
    //                  actually links against.
    ft_full_sram: u32,
    ft_su_sram: u32,
    ft_sur_sram: u32,
    kernels: []const []const u8,
};

const kernels_mcunet = [_][]const u8{
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel3x3_stride2_inplace_CHW_fpreq.c",
    "depthwise_kernel3x3_stride2_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel3x3_stride2_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel5x5_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel5x5_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel5x5_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel7x7_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel7x7_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel7x7_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel7x7_stride2_inplace_CHW_fpreq.c",
    "depthwise_kernel7x7_stride2_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel7x7_stride2_inplace_CHW_fpreq_mask.c",
};
const kernels_mbv2 = [_][]const u8{
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel3x3_stride2_inplace_CHW_fpreq.c",
    "depthwise_kernel3x3_stride2_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel3x3_stride2_inplace_CHW_fpreq_mask.c",
};
const kernels_proxyless = [_][]const u8{
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel3x3_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel5x5_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel5x5_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel5x5_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel5x5_stride2_inplace_CHW_fpreq.c",
    "depthwise_kernel5x5_stride2_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel5x5_stride2_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel7x7_stride1_inplace_CHW_fpreq.c",
    "depthwise_kernel7x7_stride1_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel7x7_stride1_inplace_CHW_fpreq_mask.c",
    "depthwise_kernel7x7_stride2_inplace_CHW_fpreq.c",
    "depthwise_kernel7x7_stride2_inplace_CHW_fpreq_bitmask.c",
    "depthwise_kernel7x7_stride2_inplace_CHW_fpreq_mask.c",
};

// Per-model sparse_bp budgets selected for codegen compatibility:
//   mcunet: 49kb (paper-default; manual_weight_idx happens to hit only
//           pointwise weights with %4-aligned partial channels)
//   mbv2:   123kb (smallest budget where no weight_update_ratio=0.125;
//           smaller budgets hit transpose_conv2d.py:313 NotImplementedError
//           for first_k_channel=7 on the 56-channel layers)
//   proxyless: 74kb (smallest budget where no 0.125 ratios)
// PEAK_MEM / pooled_offset come from the sparse_bp variant's genModel.h
// and may differ from the fwd-only triplet (sparse_bp adds backward-pass
// scratch which shifts buffer layout).
const models = [_]ModelCfg{
    .{ .name = "mcunet", .codegen_dir = "codegen-mcunet", .pooled_offset = 34592, .pooled_count = 160, .has_train_graph = 1, .peak_mem = 189800, .model_size = flash_mcunet, .ft_full_sram = 3841444, .ft_su_sram = 213136, .ft_sur_sram = 189800, .kernels = &kernels_mcunet },
    .{ .name = "mbv2", .codegen_dir = "codegen-mbv2", .pooled_offset = 45584, .pooled_count = 112, .has_train_graph = 1, .peak_mem = 247144, .model_size = flash_mbv2, .ft_full_sram = 2844756, .ft_su_sram = 510364, .ft_sur_sram = 247144, .kernels = &kernels_mbv2 },
    .{ .name = "proxyless", .codegen_dir = "codegen-proxyless", .pooled_offset = 38720, .pooled_count = 96, .has_train_graph = 1, .peak_mem = 157568, .model_size = flash_proxyless, .ft_full_sram = 2977956, .ft_su_sram = 318324, .ft_sur_sram = 157568, .kernels = &kernels_proxyless },
};

pub fn build(b: *std.Build) void {
    const target = b.resolveTargetQuery(.{
        .cpu_arch = .wasm32,
        .os_tag = .wasi,
    });
    const optimize = b.standardOptimizeOption(.{});

    inline for (models) |m| {
        buildOne(b, target, optimize, m);
    }

    // package.json is model-agnostic (the wasm loader picks which .wasm to
    // fetch at runtime). Emit it once at the install root.
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

fn buildOne(
    b: *std.Build,
    target: std.Build.ResolvedTarget,
    optimize: std.builtin.OptimizeMode,
    comptime m: ModelCfg,
) void {
    const exe_name = base_name ++ "-" ++ m.name;
    const wasm_install_path = "../pkg/" ++ base_name ++ "-" ++ m.name ++ ".wasm";

    const root_module = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });

    const lib = b.addExecutable(.{
        .name = exe_name,
        .root_module = root_module,
    });
    lib.entry = .disabled;
    lib.rdynamic = true;
    lib.initial_memory = 32 * 1024 * 1024;
    lib.max_memory = 64 * 1024 * 1024;

    root_module.addIncludePath(b.path("shim"));
    root_module.addIncludePath(b.path("vendor/tinyengine/include"));
    root_module.addIncludePath(b.path(m.codegen_dir ++ "/include"));
    root_module.addIncludePath(b.path("vendor/cmsis/Core/Include"));
    root_module.addIncludePath(b.path("vendor/cmsis/NN/Include"));

    // Per-model compile defines. POOLED_OFFSET and POOLED_COUNT are
    // consumed by src/te_model_helpers.c; HAS_TRAIN_GRAPH switches
    // te_train_step between invoke(labels) and invoke_inf() there.
    // BACKEND_NAME is just a string the wasm reports back so the UI's
    // model dropdown can verify it loaded the expected backbone.
    const c_flags = &[_][]const u8{
        "-std=gnu11",
        "-O2",
        "-DINPUT_W=128",
        "-DINPUT_H=128",
        "-DINPUT_C=3",
        "-DNUM_CLASSES=10",
        "-DPOOLED_OFFSET=" ++ std.fmt.comptimePrint("{d}", .{m.pooled_offset}),
        "-DPOOLED_COUNT=" ++ std.fmt.comptimePrint("{d}", .{m.pooled_count}),
        "-DHAS_TRAIN_GRAPH=" ++ std.fmt.comptimePrint("{d}", .{m.has_train_graph}),
        "-DPEAK_MEM_VALUE=" ++ std.fmt.comptimePrint("{d}", .{m.peak_mem}),
        "-DMODEL_SIZE_VALUE=" ++ std.fmt.comptimePrint("{d}", .{m.model_size}),
        // Paper Figure 10 peak-SRAM variants. Reported by te_get_memory()
        // so the UI can show the FT-Full vs FT-SU vs FT-SU+R comparison
        // for the active backbone.
        "-DFT_FULL_SRAM=" ++ std.fmt.comptimePrint("{d}", .{m.ft_full_sram}),
        "-DFT_SU_SRAM=" ++ std.fmt.comptimePrint("{d}", .{m.ft_su_sram}),
        "-DFT_SUR_SRAM=" ++ std.fmt.comptimePrint("{d}", .{m.ft_sur_sram}),
        "-DBACKEND_NAME=\"" ++ m.name ++ "\"",
        "-w",
        "-Wno-error",
        "-Wno-incompatible-pointer-types",
        "-Wno-pointer-sign",
        "-Wno-implicit-function-declaration",
        "-Wno-int-conversion",
        "-Wno-deprecated-non-prototype",
    };

    root_module.addCSourceFile(.{ .file = b.path("src/bridge.c"), .flags = c_flags });
    root_module.addCSourceFile(.{ .file = b.path("src/te_model_helpers.c"), .flags = c_flags });
    root_module.addCSourceFile(.{ .file = b.path("src/te_extra_backward_kernels.c"), .flags = c_flags });
    // int8input variants of group_conv_fp kernels — wrappers around the
    // vendor fp32 kernels. mcunet doesn't call them but linking the shim
    // for all models keeps the build matrix simple; unused functions get
    // dead-code-eliminated by wasm-ld.
    root_module.addCSourceFile(.{ .file = b.path("src/te_int8input_shim.c"), .flags = c_flags });

    // genModel.c + te_snapshot.c + the per-model depthwise kernels.
    root_module.addCSourceFile(.{ .file = b.path(m.codegen_dir ++ "/source/genModel.c"), .flags = c_flags });
    root_module.addCSourceFile(.{ .file = b.path(m.codegen_dir ++ "/source/te_snapshot.c"), .flags = c_flags });
    inline for (m.kernels) |f| {
        root_module.addCSourceFile(.{ .file = b.path(m.codegen_dir ++ "/source/" ++ f), .flags = c_flags });
    }

    inline for (fp_backward_sources) |f| {
        root_module.addCSourceFile(.{ .file = b.path("vendor/tinyengine/kernels/fp_backward_op/" ++ f), .flags = c_flags });
    }
    inline for (int_forward_sources) |f| {
        root_module.addCSourceFile(.{ .file = b.path("vendor/tinyengine/kernels/int_forward_op/" ++ f), .flags = c_flags });
    }
    inline for (fp_requantize_sources) |f| {
        root_module.addCSourceFile(.{ .file = b.path("vendor/tinyengine/kernels/fp_requantize_op/" ++ f), .flags = c_flags });
    }

    const install = b.addInstallFile(lib.getEmittedBin(), wasm_install_path);
    b.getInstallStep().dependOn(&install.step);
}

const fp_backward_sources = [_][]const u8{
    "add_fp.c",
    "div_fp.c",
    "group_conv_fp_kernel4_stride1_pad0.c",
    "group_conv_fp_kernel8_stride1_pad0.c",
    "group_pointwise_conv_fp.c",
    "less_fp.c",
    "log_softmax_fp.c",
    "mul_fp.c",
    "negative_fp.c",
    "nll_loss_fp.c",
    "pointwise_conv_fp.c",
    "strided_slice_3Dto3D_fp.c",
    "strided_slice_4Dto4D_fp.c",
    "sub_fp.c",
    "sum_2D_fp.c",
    "sum_3D_fp.c",
    "sum_4D_exclude_fp.c",
    "transpose_depthwise_conv_fp_kernel3_stride1_inpad1_outpad0.c",
    "transpose_depthwise_conv_fp_kernel3_stride2_inpad1_outpad1.c",
    "transpose_depthwise_conv_fp_kernel5_stride1_inpad2_outpad0.c",
    "transpose_depthwise_conv_fp_kernel5_stride2_inpad2_outpad1.c",
    "transpose_depthwise_conv_fp_kernel7_stride1_inpad3_outpad0.c",
    "transpose_depthwise_conv_fp_kernel7_stride2_inpad3_outpad1.c",
    "tte_exp_fp.c",
    "where_fp.c",
};

const int_forward_sources = [_][]const u8{
    "add.c",
    "arm_convolve_s8_4col.c",
    "arm_nn_mat_mult_kernel3_input3_s8_s16.c",
    "arm_nn_mat_mult_kernel_s8_s16_reordered_8mul.c",
    "arm_nn_mat_mult_kernel_s8_s16_reordered_oddch.c",
    "avgpooling.c",
    "concat_ch.c",
    "convolve_1x1_s8.c",
    "convolve_1x1_s8_SRAM.c",
    "convolve_1x1_s8_ch16.c",
    "convolve_1x1_s8_ch24.c",
    "convolve_1x1_s8_ch48.c",
    "convolve_1x1_s8_ch8.c",
    "convolve_1x1_s8_kbuf.c",
    "convolve_1x1_s8_oddch.c",
    "convolve_1x1_s8_skip_pad.c",
    "convolve_s8_kernel2x3_inputch3_stride2_pad1.c",
    "convolve_s8_kernel3_inputch3_stride2_pad1.c",
    "convolve_s8_kernel3_stride1_pad1.c",
    "convolve_s8_kernel3x2_inputch3_stride2_pad1.c",
    "convolve_u8_kernel3_inputch3_stride1_pad1.c",
    "convolve_u8_kernel3_inputch3_stride2_pad1.c",
    "element_mult.c",
    "fully_connected.c",
    "mat_mul_fp.c",
    "mat_mult_kernels.c",
    "maxpooling.c",
    "patchpadding_convolve_s8_kernel3_inputch3_stride2.c",
    "patchpadding_depthwise_kernel3x3_stride1_inplace_CHW.c",
    "patchpadding_depthwise_kernel3x3_stride2_inplace_CHW.c",
    "patchpadding_kbuf_convolve_s8_kernel3_inputch3_stride2.c",
    "stable_softmax.c",
    "upsample_byte.c",
};

const fp_requantize_sources = [_][]const u8{
    "add_fpreq.c",
    "convolve_1x1_s8_ch16_fpreq.c",
    "convolve_1x1_s8_ch24_fpreq.c",
    "convolve_1x1_s8_ch48_fpreq.c",
    "convolve_1x1_s8_ch8_fpreq.c",
    "convolve_1x1_s8_fpreq.c",
    "convolve_1x1_s8_fpreq_mask.c",
    "convolve_1x1_s8_fpreq_mask_partialCH.c",
    "convolve_s8_kernel3_inputch3_stride2_pad1_fpreq.c",
    "mat_mul_kernels_fpreq.c",
};
