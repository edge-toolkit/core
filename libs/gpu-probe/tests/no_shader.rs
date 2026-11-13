//! Covers the probe sets no shader compiler decides: the clears, and the blend, filter and raster probes -- their
//! formats, correct answers and shaders, and what a signature reads out of a device's answers to them.
#![cfg(test)]

use et_gpu_probe::{
    Answer, BLENDS, CLEAR_COUNT, CLEARS, FILTERS, FIXED_FUNCTION_COUNT, Format, Operation, RASTER_FORMAT, RASTER_WIDTH,
    RASTERS, Side, Signature, WEBGL2_CLEAR, WEBGL2_FIXED_FUNCTION, WEBGPU_CLEAR, WEBGPU_FIXED_FUNCTION,
    clear_reference, coverage, fixed_function_reference,
};

/// Every format the probes use, each beside its WebGPU name, the bytes a texel of it takes and its red channel's mask.
const FORMATS: [(Format, &str, u32, u32); 5] = [
    (Format::Rgba8Unorm, "rgba8unorm", 4, 0xff),
    (Format::Rgba8UnormSrgb, "rgba8unorm-srgb", 4, 0xff),
    (Format::Rgb10a2Unorm, "rgb10a2unorm", 4, 0x3ff),
    (Format::Rg11b10Ufloat, "rg11b10ufloat", 4, 0x7ff),
    (Format::Rgba16Float, "rgba16float", 8, 0xffff),
];

/// The codes each clear's red channel holds when its value is converted correctly.
const CLEAR_CODES: [u32; CLEAR_COUNT] = [10, 10, 11, 101, 188, 7, 0x3c1, 0x3c01, 0x10, 0x7c00];

/// The answer each fixed-function probe gives when computed exactly.
const FIXED_FUNCTION_ANSWERS: [u32; FIXED_FUNCTION_COUNT] =
    [50, 0x3c01, 225, 0x3eaa_aaac, 0x3a83_1200, 0x3ddd_0a89, 0x0f, 0xf0, 0x0f];

/// A signature through `interface` answering every probe with `answers`.
fn answered(interface: &'static str, answers: &[u32]) -> Signature {
    Signature::new(interface, answers.iter().copied().map(Answer::Bits).collect())
}

#[test]
fn every_format_has_its_webgpu_name_and_texel_size() {
    for (format, name, bytes, _) in FORMATS {
        assert_eq!(format.name(), name);
        assert_eq!(format.texel_bytes(), bytes);
        assert_eq!(usize::try_from(bytes).unwrap(), format.texel(1).len());
    }
}

#[test]
fn a_texel_holds_its_code_in_red_and_reads_back_the_same() {
    for (format, .., mask) in FORMATS {
        let texel = format.texel(0xabcd);
        // Each format keeps as many of the code's bits as its red channel has.
        assert_eq!(format.red(*texel.first_chunk::<4>().unwrap()), 0xabcd & mask);
    }
}

#[test]
#[expect(
    clippy::little_endian_bytes,
    reason = "WebGPU lays a texel out in little-endian order on every platform"
)]
fn a_texel_has_zero_green_and_blue_and_an_opaque_alpha() {
    assert_eq!(Format::Rgba8Unorm.texel(7), [7, 0, 0, 0xff]);
    assert_eq!(Format::Rgba8UnormSrgb.texel(7), [7, 0, 0, 0xff]);
    assert_eq!(Format::Rgb10a2Unorm.texel(7), 0xc000_0007_u32.to_le_bytes());
    assert_eq!(Format::Rg11b10Ufloat.texel(7), 7_u32.to_le_bytes());
    assert_eq!(Format::Rgba16Float.texel(7), 0x3c00_0000_0000_0007_u64.to_le_bytes());
}

#[test]
fn a_normalized_code_decodes_to_its_share_of_the_scale_and_encodes_back() {
    for (format, scale) in [(Format::Rgba8Unorm, 255), (Format::Rgb10a2Unorm, 1023)] {
        assert_eq!(format.encode(format.decode(scale)), scale);
        assert_eq!(format.encode(format.decode(3)), 3);
        // Out of range values are clamped.
        assert_eq!(format.encode(2.0), scale);
        assert_eq!(format.encode(-1.0), 0);
    }
}

#[test]
fn an_srgb_code_decodes_through_both_parts_of_the_curve_and_encodes_back() {
    for code in [0, 7, 10, 11, 128, 188, 255] {
        assert_eq!(Format::Rgba8UnormSrgb.encode(Format::Rgba8UnormSrgb.decode(code)), code);
    }
}

#[test]
fn a_small_float_decodes_its_subnormals_normals_and_infinity() {
    assert_eq!(
        Format::Rgba16Float.decode(0x0010).to_bits(),
        (1.0_f64 / 1_048_576.0).to_bits()
    );
    assert_eq!(Format::Rgba16Float.decode(0x3c00).to_bits(), 1.0_f64.to_bits());
    assert_eq!(Format::Rgba16Float.decode(0x7bff).to_bits(), 65_504.0_f64.to_bits());
    assert!(Format::Rgba16Float.decode(0x7c00).is_infinite());
    assert_eq!(
        Format::Rg11b10Ufloat.decode(0x3c1).to_bits(),
        (1.0_f64 + 1.0 / 64.0).to_bits()
    );
    assert!(Format::Rg11b10Ufloat.decode(0x7c0).is_infinite());
}

#[test]
fn a_small_float_encodes_zero_ties_and_carries_correctly() {
    assert_eq!(Format::Rgba16Float.encode(0.0), 0);
    assert_eq!(Format::Rgba16Float.encode(65_504.0), 0x7bff);
    // Halfway between 1 and the next half is a tie that goes to the even mantissa, 1.
    assert_eq!(Format::Rgba16Float.encode(1.0 + 1.0 / 2048.0), 0x3c00);
    // A mantissa that rounds up past its last value carries into the exponent.
    assert_eq!(Format::Rgba16Float.encode(2.0 - 1.0 / 8192.0), 0x4000);
    assert_eq!(Format::Rg11b10Ufloat.encode(1.0), 0x3c0);
}

#[test]
fn every_clear_converts_correctly_to_the_code_it_was_aimed_at() {
    assert_eq!(clear_reference(), CLEAR_CODES);
    assert_eq!(CLEARS.len(), CLEAR_COUNT);
    for clear in CLEARS {
        assert_ne!(clear.name, "");
        assert!(clear.value > 0.0_f64);
    }
}

#[test]
fn a_device_converting_every_clear_correctly_names_its_choices() {
    let signature = answered(WEBGPU_CLEAR, &CLEAR_CODES);
    assert_eq!(signature.completed(), CLEAR_COUNT);
    assert_eq!(signature.deviations(), 0);
    assert_eq!(
        signature.traits(),
        concat!(
            "unorm8_three_quarters=rounded unorm10_three_quarters=rounded ufloat11_rounding=rounded ",
            "half_rounding=rounded half_subnormal=kept half_overflow=infinity"
        )
    );
    assert_eq!(signature.class(), et_gpu_probe::UNRECOGNISED);
}

#[test]
fn a_device_that_truncates_and_flushes_is_told_apart() {
    let mut answers = CLEAR_CODES;
    answers[2] = 10;
    answers[3] = 100;
    answers[6] = 0x3c0;
    answers[7] = 0x3c00;
    answers[8] = 0;
    answers[9] = 0x7bff;
    let signature = answered(WEBGPU_CLEAR, &answers);
    assert_eq!(signature.deviations(), 6);
    assert_eq!(
        signature.traits(),
        concat!(
            "unorm8_three_quarters=truncated unorm10_three_quarters=truncated ufloat11_rounding=truncated ",
            "half_rounding=truncated half_subnormal=flushed half_overflow=clamped"
        )
    );
    assert!(
        signature
            .describe()
            .iter()
            .any(|line| line == "half_overflow gpu=00007bff correct=00007c00 ulps=-1")
    );
}

#[test]
fn a_clear_the_device_refused_reads_failed() {
    let mut observed: Vec<Answer> = CLEAR_CODES.iter().copied().map(Answer::Bits).collect();
    observed[6] = Answer::Refused("not renderable".to_owned());
    let signature = Signature::new(WEBGPU_CLEAR, observed);
    assert_eq!(signature.completed(), CLEAR_COUNT - 1);
    assert!(signature.traits().contains(" ufloat11_rounding=failed "));
    assert!(
        signature
            .describe()
            .contains(&"ufloat11_rounding failed: not renderable".to_owned())
    );
}

#[test]
fn every_fixed_function_probe_has_its_exact_answer() {
    assert_eq!(fixed_function_reference(), FIXED_FUNCTION_ANSWERS);
    assert_eq!(BLENDS.len() + FILTERS.len() + RASTERS.len(), FIXED_FUNCTION_COUNT);
}

#[test]
fn a_device_computing_every_fixed_function_probe_exactly_names_its_choices() {
    let signature = answered(WEBGPU_FIXED_FUNCTION, &FIXED_FUNCTION_ANSWERS);
    assert_eq!(signature.deviations(), 0);
    assert_eq!(
        signature.traits(),
        concat!(
            "blend_half_float=rounded raster_right_edge_fine=exact raster_left_edge_fine=exact ",
            "raster_right_edge_coarse=exact"
        )
    );
    let lines = signature.describe();
    assert_eq!(lines.len(), FIXED_FUNCTION_COUNT);
    assert_eq!(
        lines.first().unwrap(),
        "blend_multiply gpu=00000032 correct=00000032 ulps=+0"
    );
}

#[test]
fn a_rasterizer_that_snaps_to_eight_bits_is_told_apart() {
    let mut answers = FIXED_FUNCTION_ANSWERS;
    answers[6] = 0x07;
    answers[7] = 0xf8;
    let signature = answered(WEBGPU_FIXED_FUNCTION, &answers);
    assert_eq!(
        signature.traits(),
        concat!(
            "blend_half_float=rounded raster_right_edge_fine=snapped raster_left_edge_fine=snapped ",
            "raster_right_edge_coarse=exact"
        )
    );
}

#[test]
fn every_blend_shader_draws_its_source_over_the_whole_target() {
    for blend in BLENDS {
        let shader = blend.shader();
        let source = format!("bitcast<f32>({:#010x}u)", blend.source.to_bits());
        assert_eq!(shader.matches(&source).count(), 4);
        assert!(shader.contains("fn vertex(") && shader.contains("fn fragment("));
        assert!(!shader.contains("@binding"));
    }
    assert_eq!(
        BLENDS.map(|blend| blend.operation),
        [Operation::Multiply, Operation::Add, Operation::Add]
    );
}

#[test]
fn every_filter_shader_samples_its_coordinate_from_two_bindings() {
    for filter in FILTERS {
        let shader = filter.shader();
        assert!(shader.contains(&format!("bitcast<f32>({:#010x}u)", filter.coordinate.to_bits())));
        assert!(shader.contains("@binding(0) var probeTexture") && shader.contains("@binding(1) var probeSampler"));
        assert!(shader.contains("textureSampleLevel("));
    }
}

#[test]
fn every_raster_shader_puts_its_edge_in_clip_space_and_its_far_corner_on_its_side() {
    for raster in RASTERS {
        let shader = raster.shader();
        let across = raster.edge / 4.0 - 1.0;
        assert_eq!(shader.matches(&format!("{:#010x}u", across.to_bits())).count(), 2);
        let far: f32 = if raster.side == Side::Left { -5.0 } else { 5.0 };
        assert!(shader.contains(&format!("{:#010x}u", far.to_bits())));
    }
}

/// The GLSL ES 3.00 expression for exactly `value`.
fn glsl_literal(value: f32) -> String {
    format!("uintBitsToFloat({:#010x}u)", value.to_bits())
}

/// Check `[vertex, fragment]` is a GLSL ES 3.00 pair drawing three corners from `gl_VertexID` into `probeOut`.
fn assert_glsl_draw(program: &[String; 2]) {
    let [vertex, fragment] = program;
    assert!(vertex.starts_with("#version 300 es\n") && fragment.starts_with("#version 300 es\n"));
    assert!(vertex.contains("vec2 corners[3] = vec2[3](") && vertex.contains("corners[gl_VertexID]"));
    assert!(fragment.contains("precision highp float;") && fragment.contains("out highp vec4 probeOut;"));
}

#[test]
fn every_blend_draws_the_same_source_in_glsl() {
    for blend in BLENDS {
        let program = blend.glsl();
        assert_glsl_draw(&program);
        assert!(program[1].contains(&format!("probeOut = vec4({});", glsl_literal(blend.source))));
        assert!(!program[1].contains("uniform"));
    }
}

#[test]
fn every_filter_samples_the_same_coordinate_in_glsl() {
    for filter in FILTERS {
        let program = filter.glsl();
        assert_glsl_draw(&program);
        let point = format!("vec2({}, 0.5)", glsl_literal(filter.coordinate));
        assert!(program[1].contains("uniform highp sampler2D probeTexture;\n"));
        assert!(program[1].contains(&format!("textureLod(probeTexture, {point}, 0.0).r")));
    }
}

#[test]
fn every_raster_puts_the_same_corners_in_glsl() {
    for raster in RASTERS {
        let program = raster.glsl();
        assert_glsl_draw(&program);
        let across = glsl_literal(raster.edge / 4.0 - 1.0);
        let far = glsl_literal(if raster.side == Side::Left { -5.0 } else { 5.0 });
        let corners = format!(
            "vec2({across}, {}), vec2({across}, {}), vec2({far}, {})",
            glsl_literal(-3.0),
            glsl_literal(3.0),
            glsl_literal(0.0)
        );
        assert!(program[0].contains(&corners));
        assert!(
            raster
                .shader()
                .contains(&format!("{:#010x}u", (raster.edge / 4.0 - 1.0).to_bits()))
        );
        assert!(program[1].contains("probeOut = vec4(1.0);"));
    }
}

#[test]
fn webgl2_signatures_read_with_the_same_sets_as_webgpu_ones() {
    for (webgl2, webgpu, answers) in [
        (WEBGL2_CLEAR, WEBGPU_CLEAR, &CLEAR_CODES[..]),
        (
            WEBGL2_FIXED_FUNCTION,
            WEBGPU_FIXED_FUNCTION,
            &FIXED_FUNCTION_ANSWERS[..],
        ),
    ] {
        let (through_webgl2, through_webgpu) = (answered(webgl2, answers), answered(webgpu, answers));
        assert_eq!(through_webgl2.results(), through_webgpu.results());
        assert_eq!(through_webgl2.traits(), through_webgpu.traits());
        assert_eq!(through_webgl2.deviations(), 0);
    }
}

#[test]
fn coverage_sets_a_bit_for_each_texel_with_red() {
    let texel_bytes = usize::try_from(RASTER_FORMAT.texel_bytes()).unwrap();
    let width = usize::try_from(RASTER_WIDTH).unwrap();
    let mut texels = vec![0_u8; texel_bytes * width];
    for texel in [0, 2, 7] {
        texels[texel * texel_bytes] = 0xff;
    }
    // Alpha and the other channels say nothing about coverage.
    texels[texel_bytes + 3] = 0xff;
    assert_eq!(coverage(&texels), 0b1000_0101);
    assert_eq!(coverage(&[]), 0);
}

#[cfg(feature = "registry")]
#[test]
fn the_registry_holds_no_signature_of_the_new_sets_yet() {
    assert_eq!(answered(WEBGPU_CLEAR, &CLEAR_CODES).identify(), Vec::<&str>::new());
    assert_eq!(
        answered(WEBGPU_FIXED_FUNCTION, &FIXED_FUNCTION_ANSWERS).identify(),
        Vec::<&str>::new()
    );
    assert_eq!(answered(WEBGL2_CLEAR, &CLEAR_CODES).identify(), Vec::<&str>::new());
}
