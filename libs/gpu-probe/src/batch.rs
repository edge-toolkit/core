//! The probes as fragment shaders that each evaluate a batch of them, picking a probe by the pixel being drawn.
//!
//! A batch is drawn into a target as wide as the batch, one pixel a probe, and the row read back at once, so one
//! compilation answers every probe in it. A compiler may treat an expression differently beside others, so a signature
//! taken with the whole set in one shader is filed apart from one taken with a shader a probe: [`WEBGL2_COMBINED`]
//! against [`WEBGL2`], [`WEBGL1_COMBINED`] against [`WEBGL1`]. The shaders are the research page's, so a signature
//! taken with them is the one that page takes on the same device.

use crate::{Answer, PROBES, Signature, WEBGL2};

/// A batch's `main`, from its opening to the pick of the probe the pixel being drawn evaluates.
macro_rules! batch_main {
    () => {
        concat!(
            "void main() {\n",
            glsl_inputs!(),
            "\n    int idx = int(gl_FragCoord.x);\n    float value = 0.0;\n"
        )
    };
}

/// The declarations a GLSL ES 1.00 batch opens with.
macro_rules! es100_open {
    () => {
        "precision highp float;\nprecision mediump int;\nuniform float probeIn[8];\n"
    };
}

/// [`WEBGL2`] where every probe GLSL can express ran in the one shader, not in a shader of its own.
pub const WEBGL2_COMBINED: &str = "webgl2-combined";

/// The interface a signature was measured through, where the probes ran as GLSL ES 1.00 fragment shaders on WebGL 1.
///
/// WebGL 1 is offered by some browsers that withhold WebGL 2 and WebGPU, the Tor Browser among them.
pub const WEBGL1: &str = "webgl1";

/// [`WEBGL1`] where every probe GLSL can express ran in the one shader, not in a shader of its own.
pub const WEBGL1_COMBINED: &str = "webgl1-combined";

/// What a probe GLSL has no builtin for is refused with, without a shader being compiled for it.
pub const NO_GLSL_BUILTIN: &str = "GLSL has no builtin for it";

/// The corners of the triangle a GLSL ES 1.00 batch is drawn with, as `x, y` pairs, one covering the whole target.
pub const TRIANGLE: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];

/// The vertex shader a GLSL ES 3.00 batch is drawn with, its triangle's corners placed from `gl_VertexID`.
const BATCH_VERTEX_SHADER: &str = "#version 300 es
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

/// The vertex shader a GLSL ES 1.00 batch is drawn with, its triangle's corners read from `aPos`.
const WEBGL1_VERTEX_SHADER: &str = "attribute vec2 aPos;
void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
}
";

/// The part of a GLSL ES 3.00 batch before its probes.
const ES300_HEAD: &str = concat!(
    "#version 300 es\nprecision highp float;\nprecision highp int;\nuniform float probeIn[8];\nout vec4 probeOut;\n\n",
    batch_main!()
);

/// The part of a GLSL ES 3.00 batch after its probes, writing the bits of the pixel's result lowest byte first.
const ES300_TAIL: &str = concat!("    uint bits = floatBitsToUint(value);\n", glsl_write_bits!());

/// The part of a GLSL ES 1.00 batch before its probes, for a target that holds the result as a float.
const ES100_FLOAT_HEAD: &str = concat!(es100_open!(), "\n", batch_main!());

/// The part of a GLSL ES 1.00 batch after its probes, writing the pixel's result to its red as a float.
const ES100_FLOAT_TAIL: &str = "    gl_FragColor = vec4(value, 0.0, 0.0, 0.0);\n}\n";

/// The part of a GLSL ES 1.00 batch before its probes, for an 8-bit target, with the function that packs a result.
///
/// `packFloatBits` works out the IEEE-754 single-precision bits of a value with only exact operations -- powers of two,
/// `floor`, and `mod` of whole numbers -- so for a value the GPU holds it is a lossless re-encoding of the bits.
const ES100_PACKED_HEAD: &str = concat!(
    es100_open!(),
    "
vec4 packFloatBits(float value) {
    if (value == 0.0) {
        return vec4(0.0);
    }
    float sign = 0.0;
    float v = value;
    if (v < 0.0) {
        sign = 1.0;
        v = -v;
    }
    float exponent = floor(log2(v));
    float biased;
    float mantissaBits;
    if (exponent < -126.0) {
        biased = 0.0;
        mantissaBits = floor(v / exp2(-126.0) * exp2(23.0) + 0.5);
    } else {
        float m = v / exp2(exponent);
        if (m < 1.0) {
            exponent -= 1.0;
            m = v / exp2(exponent);
        }
        if (m >= 2.0) {
            exponent += 1.0;
            m = v / exp2(exponent);
        }
        biased = exponent + 127.0;
        mantissaBits = floor((m - 1.0) * exp2(23.0) + 0.5);
        if (mantissaBits >= exp2(23.0)) {
            mantissaBits = 0.0;
            biased += 1.0;
        }
    }
    float byte3 = sign * 128.0 + floor(biased / 2.0);
    float byte2 = mod(biased, 2.0) * 128.0 + floor(mantissaBits / exp2(16.0));
    float rem = mod(mantissaBits, exp2(16.0));
    float byte1 = floor(rem / exp2(8.0));
    float byte0 = mod(rem, exp2(8.0));
    return vec4(byte0, byte1, byte2, byte3) / 255.0;
}

",
    batch_main!()
);

/// The part of a GLSL ES 1.00 batch after its probes, writing the bits of the pixel's result lowest byte first.
const ES100_PACKED_TAIL: &str = "    gl_FragColor = packFloatBits(value);\n}\n";

/// The GLSL a batch is written in, and how its results leave the shader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Glsl {
    /// GLSL ES 3.00, for WebGL 2, each result's bits taken with `floatBitsToUint` into an 8-bit RGBA pixel.
    ///
    /// The bits are written lowest byte first as the pixel's red, green, blue and alpha.
    Es300,
    /// GLSL ES 1.00, for WebGL 1, each result written as it is to the red of a 32-bit float pixel.
    ///
    /// GLSL ES 1.00 has no `floatBitsToUint`, so a float target is how its bits are read back unchanged.
    Es100Float,
    /// GLSL ES 1.00, for a WebGL 1 that will not read a float pixel back, each result packed into an 8-bit RGBA pixel.
    ///
    /// The bits are worked out with exact arithmetic and written lowest byte first as the red, green, blue and alpha.
    Es100Packed,
}

impl Glsl {
    /// The vertex shader a batch in this GLSL is drawn with: one triangle covering the whole target.
    ///
    /// GLSL ES 3.00 places the triangle's corners from `gl_VertexID`. GLSL ES 1.00 has none, so they are read from the
    /// attribute `aPos`, which the caller fills with [`TRIANGLE`].
    #[must_use]
    pub const fn vertex_shader(self) -> &'static str {
        match self {
            Self::Es300 => BATCH_VERTEX_SHADER,
            Self::Es100Float | Self::Es100Packed => WEBGL1_VERTEX_SHADER,
        }
    }

    /// The fragment shader evaluating the probes at `probes`, positions in output order.
    ///
    /// The first probe is evaluated at the pixel whose x is 0, and each after it one pixel on.
    #[must_use]
    pub fn fragment_shader(self, probes: &[usize]) -> String {
        let branches: Vec<String> = probes
            .iter()
            .enumerate()
            .filter_map(|(pixel, probe)| PROBES.get(*probe).map(|named| (pixel, named.2)))
            .map(|(pixel, expression)| format!("    if (idx == {pixel}) {{ value = {expression}; }}\n"))
            .collect();
        let (head, tail) = match self {
            Self::Es300 => (ES300_HEAD, ES300_TAIL),
            Self::Es100Float => (ES100_FLOAT_HEAD, ES100_FLOAT_TAIL),
            Self::Es100Packed => (ES100_PACKED_HEAD, ES100_PACKED_TAIL),
        };
        [head, &branches.concat(), tail].concat()
    }

    /// The interface a signature taken in this GLSL is filed under, `combined` when every probe ran in one shader.
    const fn interface(self, combined: bool) -> &'static str {
        match (self, combined) {
            (Self::Es300, false) => WEBGL2,
            (Self::Es300, true) => WEBGL2_COMBINED,
            (Self::Es100Float | Self::Es100Packed, false) => WEBGL1,
            (Self::Es100Float | Self::Es100Packed, true) => WEBGL1_COMBINED,
        }
    }
}

/// Why one batch gave no answers.
#[derive(Debug)]
#[non_exhaustive]
pub enum Batch<Failure> {
    /// The browser would not compile or link the batch's shader, with its log.
    ///
    /// The batch is halved until the probe the browser will not take is alone, and that probe refused.
    Refused(String),
    /// The batch could not be drawn or read for another reason, which ends the measurement.
    Failed(Failure),
}

/// What a device answers to the probe set as fragment shaders in `glsl`, each batch drawn and read back by `draw`.
///
/// `draw` is handed a batch's fragment shader and how many probes it evaluates, and gives back the bits each pixel
/// holds, in order. Every probe GLSL has an expression for goes to it, a shader each, or with `combined` all in one
/// shader; a combined shader the browser refuses is halved, and each half tried again, until the probe it will not
/// take is alone. That keeps the rest in as few shaders as the research page does, so the two group the probes alike. A
/// probe GLSL has no builtin for is refused without a shader.
///
/// # Errors
///
/// Whatever [`Batch::Failed`] `draw` gives, which ends the measurement.
pub fn batched<Failure, Draw>(glsl: Glsl, combined: bool, mut draw: Draw) -> Result<Signature, Failure>
where
    Draw: FnMut(&str, usize) -> Result<Vec<u32>, Batch<Failure>>,
{
    let expressed: Vec<usize> = (0..PROBES.len()).filter(|probe| has_builtin(*probe)).collect();
    let mut answered = run(glsl, &expressed, !combined, &mut draw)?.into_iter();
    let observed = (0..PROBES.len())
        .map(|probe| {
            if has_builtin(probe) {
                answered
                    .next()
                    .unwrap_or_else(|| Answer::Refused("no pixel was read back for it".to_owned()))
            } else {
                Answer::Refused(NO_GLSL_BUILTIN.to_owned())
            }
        })
        .collect();
    Ok(Signature::new(glsl.interface(combined), observed))
}

/// Whether GLSL has a builtin for the probe at `probe`: every one but `fma`, which GLSL ES has no function for.
fn has_builtin(probe: usize) -> bool {
    PROBES.get(probe).is_some_and(|named| !named.2.starts_with("fma("))
}

/// The answers to the probes at `probes`, a shader each when `split`, or otherwise as few shaders as will compile.
fn run<Failure, Draw>(glsl: Glsl, probes: &[usize], split: bool, draw: &mut Draw) -> Result<Vec<Answer>, Failure>
where
    Draw: FnMut(&str, usize) -> Result<Vec<u32>, Batch<Failure>>,
{
    if split && probes.len() > 1 {
        let mut answers = Vec::with_capacity(probes.len());
        for probe in probes.chunks(1) {
            answers.extend(run(glsl, probe, true, draw)?);
        }
        return Ok(answers);
    }
    match draw(&glsl.fragment_shader(probes), probes.len()) {
        Ok(bits) => Ok(bits.into_iter().map(Answer::Bits).collect()),
        Err(Batch::Failed(failure)) => Err(failure),
        Err(Batch::Refused(log)) if probes.len() <= 1 => Ok(vec![Answer::Refused(log)]),
        Err(Batch::Refused(_)) => {
            // The left half is the smaller when the count is odd, as the research page halves a batch.
            let (left, right) = probes.split_at(probes.len() >> 1_u32);
            let mut answers = run(glsl, left, false, draw)?;
            answers.extend(run(glsl, right, false, draw)?);
            Ok(answers)
        }
    }
}
