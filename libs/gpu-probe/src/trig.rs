//! `sin` and `cos` of angles past the first turn and a half, where the hardware's own reduction of the angle shows.
//!
//! A GPU evaluates WGSL's `sin` and `cos` on a special-function unit, which takes the angle in turns rather than
//! radians and reduces it to one turn itself. NVIDIA's Maxwell GPUs reduce it in an instruction of their own before the
//! unit runs, and later generations inside the unit; the two agree on small angles, as the shader probes' `sin` and
//! `cos` of 0.1 do, and part from each other past about 4.5 radians. A Quadro M2000 answers these closer to the correct
//! value than an RTX A1000 or a GB10, which answer alike.
#![expect(
    clippy::single_call_fn,
    reason = "each step of writing the set's shaders and reading its answers is a named function"
)]

use crate::{nearest, wgsl_compute};

/// The interface a signature was measured through, `sin` or `cos` of one angle per probe.
///
/// Each probe ran in a WGSL compute shader on WebGPU.
pub const WEBGPU_TRIG: &str = "webgpu-trig";

/// [`WEBGPU_TRIG`] as the registry files it, each probe having run in a shader of its own.
pub const WEBGPU_TRIG_SPLIT: &str = "webgpu-trig-split";

/// How many probes the set makes: `sin` and `cos` of each of [`TRIG_INPUTS`].
pub const TRIG_COUNT: usize = 16;

/// The angles, in radians, handed to each shader in a storage buffer so that no expression is folded at compile time.
///
/// They are `0.11 + 0.37 * i` for i = 12, 15, 17, 19, 21, 25, 28 and 30, as f32: 4.55 to 11.21, each one where the PTX
/// `sin.approx` or `cos.approx` of a Quadro M2000 was seen to answer otherwise than an RTX A1000's.
pub const TRIG_INPUTS: [f32; 8] = [
    f32::from_bits(0x4091_999a),
    f32::from_bits(0x40b5_1eb8),
    f32::from_bits(0x40cc_cccd),
    f32::from_bits(0x40e4_7ae1),
    f32::from_bits(0x40fc_28f6),
    f32::from_bits(0x4115_c28f),
    f32::from_bits(0x4127_851f),
    f32::from_bits(0x4133_5c29),
];

/// Each probe's name and the WGSL expression it evaluates, in output order: every `sin`, then every `cos`.
const TRIG_PROBES: [(&str, &str); TRIG_COUNT] = [
    ("sin_4_55", "sin(probeIn[0])"),
    ("sin_5_66", "sin(probeIn[1])"),
    ("sin_6_40", "sin(probeIn[2])"),
    ("sin_7_14", "sin(probeIn[3])"),
    ("sin_7_88", "sin(probeIn[4])"),
    ("sin_9_36", "sin(probeIn[5])"),
    ("sin_10_47", "sin(probeIn[6])"),
    ("sin_11_21", "sin(probeIn[7])"),
    ("cos_4_55", "cos(probeIn[0])"),
    ("cos_5_66", "cos(probeIn[1])"),
    ("cos_6_40", "cos(probeIn[2])"),
    ("cos_7_14", "cos(probeIn[3])"),
    ("cos_7_88", "cos(probeIn[4])"),
    ("cos_9_36", "cos(probeIn[5])"),
    ("cos_10_47", "cos(probeIn[6])"),
    ("cos_11_21", "cos(probeIn[7])"),
];

/// Each probe's name.
pub(crate) fn names() -> Vec<&'static str> {
    TRIG_PROBES.iter().map(|probe| probe.0).collect()
}

/// Each probe's name beside the WGSL compute shader that evaluates it, in output order.
///
/// Every shader reads [`TRIG_INPUTS`] from an 8-element `f32` storage buffer at binding 0 and writes its result's bits
/// to a 1-element `u32` storage buffer at binding 1, as each of [`shaders`](crate::shaders) does.
#[must_use]
pub fn trig_shaders() -> Vec<(&'static str, String)> {
    TRIG_PROBES
        .iter()
        .map(|probe| {
            (
                probe.0,
                wgsl_compute(("", "u32"), "", &[format!("bitcast<u32>({})", probe.1)]),
            )
        })
        .collect()
}

/// The bits of each probe's correctly rounded answer, in output order.
#[must_use]
pub fn trig_reference() -> Vec<u32> {
    let angles = TRIG_INPUTS.map(f64::from);
    angles
        .iter()
        .map(|angle| nearest(angle.sin()).to_bits())
        .chain(angles.iter().map(|angle| nearest(angle.cos()).to_bits()))
        .collect()
}
