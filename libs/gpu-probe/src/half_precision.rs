//! The probe set again in `f16`, through WGSL's `shader-f16` feature.
//!
//! Half precision leaves an implementation more room than single precision does, and GPUs use it: one computes `f16` in
//! hardware of its own, another widens it to `f32` and rounds each result back, and they flush subnormals and round the
//! mathematical functions differently. The probes are the ones [`shaders`](crate::shaders) gives, by the same names, on
//! inputs chosen for `f16`: a value that falls exactly halfway in `f32` does not in `f16`, and no `f16` value squares
//! to one that does, so the multiply-add takes two factors rather than one value squared.

use half::f16;

use crate::{PROBE_COUNT, PROBES, dot_in_order, functions, in_order, swamped_sum, wgsl_compute};

/// The interface a signature was measured through, where the probes ran as `f16` WGSL compute shaders on WebGPU.
///
/// A device without `shader-f16` refuses every probe of it, which is what its signature then records.
pub const WEBGPU_F16: &str = "webgpu-f16";

/// [`WEBGPU_F16`] where every probe ran in the one shader [`f16_combined_shader`] gives, not in a shader of its own.
pub const WEBGPU_F16_COMBINED: &str = "webgpu-f16-combined";

/// The `f16` probes' inputs, handed to each shader as `f32` in a storage buffer, each a value `f16` holds exactly.
///
/// In order: the two factors of the multiply-add, `1 + 2^-5` and `1 + 2^-6`, whose product falls exactly halfway
/// between two `f16` values; the negation of that product rounded to `f16`; `2^11`, above which adding 1 is lost; 1;
/// the `f16` nearest 0.1; 3; and the smallest normal `f16`, `2^-14`.
pub const F16_INPUTS: [f32; 8] = [
    1.031_25_f32,
    1.015_625_f32,
    -1.046_875_f32,
    2048.0_f32,
    1.0_f32,
    f16::from_bits(0x2e66).to_f32_const(),
    3.0_f32,
    f16::MIN_POSITIVE.to_f32_const(),
];

/// The WGSL that names each of [`F16_INPUTS`] as the `f16` it holds, and takes the sum some probes use.
const F16_SHADER_INPUTS: &str = "    let left = f16(probeIn[0]);
    let right = f16(probeIn[1]);
    let negatedProduct = f16(probeIn[2]);
    let big = f16(probeIn[3]);
    let one = f16(probeIn[4]);
    let angle = f16(probeIn[5]);
    let three = f16(probeIn[6]);
    let tiny = f16(probeIn[7]);
    let oneHalf = 0.5h;

    var halfSum : f16 = big;
    for (var turn : u32 = 0u; turn < 4u; turn = turn + 1u) {
        halfSum = halfSum + one;
    }

";

/// The WGSL expression each probe evaluates in `f16`, in the output order of the probe set's names.
const F16_EXPRESSIONS: [&str; PROBE_COUNT] = [
    "left * right + negatedProduct",
    "fma(left, right, negatedProduct)",
    "halfSum - big",
    "one / three",
    "tiny * oneHalf",
    "sqrt(three)",
    "inverseSqrt(three)",
    "sin(angle)",
    "cos(angle)",
    "tan(angle)",
    "exp(angle)",
    "log(three)",
    "pow(three, angle)",
    "atan2(angle, three)",
    "tanh(angle)",
    "dot(vec4<f16>(big, one, one, -big), vec4<f16>(one, one, one, one))",
];

/// For each `f16` probe with more than one legal exact answer: its name, then each answer's bits beside its meaning.
///
/// The answers are the ones the `f32` set names, in `f16`: a fused multiply-add keeps the product's `2^-11`, and the
/// sums of `2^11` and ones lose each one added alone, the tie rounding to the even `2^11`. A device that widens `f16`
/// to `f32` and rounds back only at the end answers as though it had fused and regrouped.
pub(crate) const F16_CHOICES: [(&str, &[(u32, &str)]); 5] = [
    ("multiply_add", &[(0, "rounded_twice"), (0x1000, "fused")]),
    ("fma", &[(0, "rounded_twice"), (0x1000, "fused")]),
    (
        "swamped_sum",
        &[(0, "in_order"), (0x4400, "reassociated"), (0x4000, "paired")],
    ),
    ("subnormal_product", &[(0x0200, "kept"), (0, "flushed")]),
    (
        "dot",
        &[(0, "in_order"), (0x4000, "reassociated"), (0x3c00, "pairwise")],
    ),
];

/// Each probe's name beside the `f16` WGSL compute shader that evaluates it, in output order.
///
/// Each is bound as each of [`shaders`](crate::shaders) is, reading [`F16_INPUTS`] in place of the `f32` inputs, and
/// writes its result's 16 bits to the low half of its word. It opens with `enable f16;`, so a device has to have been
/// opened with `shader-f16` to compile it.
#[must_use]
pub fn f16_shaders() -> Vec<(&'static str, String)> {
    PROBES
        .iter()
        .zip(F16_EXPRESSIONS)
        .map(|(probe, expression)| (probe.0, f16_compute_shader(&[expression])))
        .collect()
}

/// The one `f16` WGSL compute shader that evaluates every probe, writing each result's bits in output order.
///
/// It is to [`f16_shaders`] as [`combined_shader`](crate::combined_shader) is to the `f32` ones, and a signature taken
/// with it is filed under [`WEBGPU_F16_COMBINED`].
#[must_use]
pub fn f16_combined_shader() -> String {
    f16_compute_shader(&F16_EXPRESSIONS)
}

/// An `f16` WGSL compute shader that writes the bits of each of `expressions` to the low half of a word of its own.
///
/// The word is a `vec2<f16>` holding the result and a zero: WGSL has no 16-bit integer to take the bits as, and a
/// conversion to `f32` and back would let the device flush a subnormal result on the way.
fn f16_compute_shader(expressions: &[&str]) -> String {
    let words: Vec<String> = expressions
        .iter()
        .map(|expression| format!("vec2<f16>({expression}, 0.0h)"))
        .collect();
    wgsl_compute(("enable f16;\n", "vec2<f16>"), F16_SHADER_INPUTS, &words)
}

/// The bits of `value` rounded to the nearest `f16`, as the word a probe writes them to.
fn bits(value: f16) -> u32 {
    u32::from(value.to_bits())
}

/// The bits of `value` rounded once to the nearest `f16`.
fn nearest(value: f64) -> u32 {
    bits(f16::from_f64(value))
}

/// The bits of each `f16` probe's correctly rounded answer, in output order.
///
/// The exact probes are evaluated as written, one `f16` operation at a time, each correctly rounded. The mathematical
/// functions are evaluated in `f64` and rounded once to `f16`.
#[must_use]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "the probes are f16 arithmetic, which rounds and never panics, as f32 arithmetic does"
)]
pub fn f16_reference() -> [u32; PROBE_COUNT] {
    let [left, right, negated_product, big, one, angle, three, tiny] = F16_INPUTS.map(f16::from_f32);
    let [wide_left, wide_right, wide_negated] = [left, right, negated_product].map(f64::from);
    in_order(
        [
            bits(left * right + negated_product),
            nearest(wide_left.mul_add(wide_right, wide_negated)),
            bits(swamped_sum(big, one) - big),
            bits(one / three),
            bits(tiny * f16::from_f32(0.5)),
        ],
        functions(f64::from(angle), f64::from(three)).map(nearest),
        bits(dot_in_order(big, one, f16::ZERO)),
    )
}
