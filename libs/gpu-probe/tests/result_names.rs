//! Covers the names each interface's results take, which a file of submissions leaves out and the registry restores.
#![cfg(test)]

use et_gpu_probe::{
    CANVAS, CANVAS_BROWSERLEAKS, CLEARS, WEBGL2_CLEAR, WEBGL2_FIXED_FUNCTION, WEBGPU, WEBGPU_CLEAR, WEBGPU_F16,
    WEBGPU_FIXED_FUNCTION, result_names,
};
use pretty_assertions::assert_eq;

/// The shader probes' names, in output order.
const SHADER_PROBES: [&str; 16] = [
    "multiply_add",
    "fma",
    "swamped_sum",
    "divide",
    "subnormal_product",
    "sqrt",
    "inverse_sqrt",
    "sin",
    "cos",
    "tan",
    "exp",
    "log",
    "pow",
    "atan2",
    "tanh",
    "dot",
];

/// The fixed-function probes' names, in output order: the blends, then the filters, then the rasters.
const FIXED_FUNCTION_PROBES: [&str; 9] = [
    "blend_multiply",
    "blend_half_float",
    "blend_srgb",
    "filter_third",
    "filter_fine",
    "filter_srgb",
    "raster_right_edge_fine",
    "raster_left_edge_fine",
    "raster_right_edge_coarse",
];

#[test]
fn the_canvas_fingerprints_name_their_hash_words() {
    assert_eq!(result_names(CANVAS), ["lane0", "lane1", "lane2", "lane3"]);
    assert_eq!(result_names(CANVAS_BROWSERLEAKS), ["md5"]);
}

#[test]
fn the_clears_are_named_for_each_clear() {
    let names: Vec<&str> = CLEARS.iter().map(|clear| clear.name).collect();
    assert_eq!(result_names(WEBGPU_CLEAR), names);
    assert_eq!(result_names(WEBGL2_CLEAR), names);
}

#[test]
fn the_fixed_function_sets_name_every_blend_filter_and_raster() {
    assert_eq!(result_names(WEBGPU_FIXED_FUNCTION), FIXED_FUNCTION_PROBES);
    assert_eq!(result_names(WEBGL2_FIXED_FUNCTION), FIXED_FUNCTION_PROBES);
}

#[test]
fn every_other_interface_names_the_shader_probes() {
    for interface in [WEBGPU, WEBGPU_F16, "webgpu-split", "webgl1-combined", "a-new-mechanism"] {
        assert_eq!(result_names(interface), SHADER_PROBES);
    }
}
