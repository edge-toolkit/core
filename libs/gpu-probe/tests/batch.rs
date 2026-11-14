//! Covers the probes as batched fragment shaders: the GLSL each dialect writes, how a split and a combined measurement
//! group the probes, and how a refused shader is halved until the probe it will not take is alone.
#![cfg(test)]

use et_gpu_probe::{
    Batch, Glsl, NO_GLSL_BUILTIN, PROBE_COUNT, TRIANGLE, WEBGL1, WEBGL1_COMBINED, WEBGL2, WEBGL2_COMBINED, batched,
};

/// Where `fma`, which GLSL has no builtin for, sits in output order.
const FMA: usize = 1;

/// Where `tanh`, which GLSL ES 1.00 has no builtin for, sits in output order.
const TANH: usize = 14;

/// What a draw that never fails gives up when it fails.
type Never = core::convert::Infallible;

/// The probe expressions a batch's fragment shader picks between, in pixel order.
fn picked(shader: &str) -> Vec<&str> {
    shader
        .lines()
        .filter_map(|line| line.trim().strip_prefix("if (idx == "))
        .filter_map(|rest| rest.split_once("{ value = "))
        .map(|(_, expression)| expression.trim_end_matches("; }"))
        .collect()
}

/// A draw answering each pixel with the bits `0xb0 + pixel`, logging how wide each batch it was handed was.
fn counting(widths: &mut Vec<usize>) -> impl FnMut(&str, usize) -> Result<Vec<u32>, Batch<Never>> + '_ {
    move |_, width| {
        widths.push(width);
        Ok((0..width)
            .map(|pixel| 0xb0_u32.saturating_add(u32::try_from(pixel).unwrap()))
            .collect())
    }
}

#[test]
fn each_dialect_writes_a_shader_that_picks_each_probe_by_its_pixel() {
    let es300 = Glsl::Es300.fragment_shader(&[0, 3]);
    assert!(es300.starts_with("#version 300 es\n"));
    assert!(es300.contains("uint bits = floatBitsToUint(value);"));
    assert_eq!(picked(&es300), ["ulpBase * ulpBase + negatedSquare", "one / three"]);
    let float = Glsl::Es100Float.fragment_shader(&[5]);
    assert!(!float.contains("#version") && float.contains("gl_FragColor = vec4(value, 0.0, 0.0, 0.0);"));
    assert_eq!(picked(&float), ["sqrt(three)"]);
    let packed = Glsl::Es100Packed.fragment_shader(&[5]);
    assert!(packed.contains("vec4 packFloatBits(float value) {"));
    assert!(packed.contains("gl_FragColor = packFloatBits(value);"));
    // Every dialect names the inputs, and takes the sum, before it picks a probe.
    for shader in [es300, float, packed] {
        let sum = shader.find("sum = sum + one;").unwrap();
        assert!(sum < shader.find("int idx = int(gl_FragCoord.x);").unwrap(), "{shader}");
    }
}

#[test]
fn a_position_past_the_last_probe_picks_nothing() {
    assert_eq!(picked(&Glsl::Es300.fragment_shader(&[PROBE_COUNT])), Vec::<&str>::new());
}

#[test]
fn each_dialect_draws_with_the_vertex_shader_its_glsl_can_compile() {
    assert!(Glsl::Es300.vertex_shader().contains("gl_VertexID"));
    for glsl in [Glsl::Es100Float, Glsl::Es100Packed] {
        assert!(glsl.vertex_shader().starts_with("attribute vec2 aPos;"));
    }
    assert_eq!(TRIANGLE.len(), 6);
}

#[test]
fn a_split_measurement_draws_each_probe_glsl_has_a_builtin_for_alone() {
    let mut widths = Vec::new();
    let signature = batched(Glsl::Es300, false, counting(&mut widths)).unwrap();
    assert_eq!(widths, [1; PROBE_COUNT - 1]);
    assert_eq!(signature.interface(), WEBGL2);
    let results = signature.results();
    assert_eq!(results.len(), PROBE_COUNT);
    assert_eq!(results.get(FMA), Some(&("fma", "????????".to_owned())));
    assert!(
        results
            .iter()
            .filter(|result| result.0 != "fma")
            .all(|result| result.1 == "000000b0")
    );
}

#[test]
fn a_combined_measurement_draws_every_probe_glsl_has_a_builtin_for_at_once() {
    let mut widths = Vec::new();
    let signature = batched(Glsl::Es300, true, counting(&mut widths)).unwrap();
    assert_eq!(widths, [PROBE_COUNT - 1]);
    assert_eq!(signature.interface(), WEBGL2_COMBINED);
    let results = signature.results();
    assert_eq!(results.first().unwrap().1, "000000b0");
    assert_eq!(results.get(2).map(|result| result.1.as_str()), Some("000000b1"));
    assert_eq!(results.last().unwrap().1, "000000be");
}

#[test]
fn a_probe_glsl_has_no_builtin_for_is_refused_without_a_shader() {
    let signature = batched(Glsl::Es300, false, counting(&mut Vec::new())).unwrap();
    assert!(
        signature
            .describe()
            .iter()
            .any(|line| line.contains("fma") && line.contains(NO_GLSL_BUILTIN)),
        "{:?}",
        signature.describe()
    );
}

#[test]
fn a_refused_combined_shader_is_halved_until_the_probe_it_will_not_take_is_alone() {
    let mut widths = Vec::new();
    let signature = batched(Glsl::Es100Float, true, |shader: &str, width| {
        widths.push(width);
        if shader.contains("tanh(") {
            return Err(Batch::<Never>::Refused("no tanh".to_owned()));
        }
        Ok(vec![0; width])
    })
    .unwrap();
    // Fifteen probes halve to seven and eight, the eight holding tanh to four and four, then two, then one each.
    assert_eq!(widths, [15, 7, 8, 4, 4, 2, 2, 1, 1]);
    assert_eq!(signature.interface(), WEBGL1_COMBINED);
    let results = signature.results();
    assert_eq!(results.get(TANH).map(|result| result.1.as_str()), Some("????????"));
    assert_eq!(signature.completed(), PROBE_COUNT - 2);
}

#[test]
fn a_probe_a_split_measurement_cannot_compile_is_refused_with_the_log() {
    let signature = batched(Glsl::Es100Packed, false, |shader: &str, width| {
        if shader.contains("tanh(") {
            return Err(Batch::<Never>::Refused("tanh: no such function".to_owned()));
        }
        Ok(vec![0; width])
    })
    .unwrap();
    assert_eq!(signature.interface(), WEBGL1);
    assert!(
        signature
            .describe()
            .iter()
            .any(|line| line.contains("tanh: no such function"))
    );
}

#[test]
fn a_failed_draw_ends_the_measurement() {
    for combined in [false, true] {
        let failed = batched(Glsl::Es300, combined, |_: &str, _| Err(Batch::Failed("lost")));
        assert_eq!(failed.err(), Some("lost"));
    }
    // A failure after a refusal ends it too, from whichever half it comes.
    let mut draws = 0_u32;
    let failed = batched(Glsl::Es300, true, |_: &str, _| {
        draws += 1;
        if draws == 1 {
            Err(Batch::Refused("too big".to_owned()))
        } else {
            Err(Batch::Failed("lost"))
        }
    });
    assert_eq!(failed.err(), Some("lost"));
    let mut draws = 0_u32;
    let failed = batched(Glsl::Es300, true, |_: &str, width| {
        draws += 1;
        match draws {
            1 => Err(Batch::Refused("too big".to_owned())),
            2 => Ok(vec![0; width]),
            _ => Err(Batch::Failed("lost")),
        }
    });
    assert_eq!(failed.err(), Some("lost"));
}

#[test]
fn a_probe_no_pixel_was_read_back_for_is_refused() {
    let signature = batched(Glsl::Es300, true, |_: &str, _| Ok::<_, Batch<Never>>(vec![7])).unwrap();
    assert_eq!(signature.completed(), 1);
    assert!(
        signature
            .describe()
            .iter()
            .any(|line| line.contains("no pixel was read back for it"))
    );
}
