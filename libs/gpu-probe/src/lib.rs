//! Numeric probes that tell one GPU's floating-point behaviour from another's.
//!
//! A shading language leaves an implementation room on most floating-point operations: whether a multiply and an
//! add are fused, the order a sum is taken in, whether a subnormal survives, and how closely each mathematical
//! function is rounded. Two GPUs can therefore give different bits for the same expression and both be correct.
//! This crate holds a fixed set of such expressions, the WGSL that evaluates each, and the correctly rounded
//! answer, and reads what a device returned as a [`Signature`]: which choices it made, how far each result sits
//! from the correctly rounded one, and which known GPU, if any, answers the same way.
//!
//! It drives no GPU itself, so the same probes serve a browser's WebGPU, a native host's, and a caller with none.
//! Each probe is a shader of its own: a device that refuses one expression still answers the rest, and a
//! signature with gaps in it identifies a device better than no signature.

/// How many expressions the probe set evaluates.
pub const PROBE_COUNT: usize = 16;

/// The probes' inputs, handed to each shader in a storage buffer so that no expression is folded at compile time.
///
/// In order: `1 + 2^-12`, whose square falls exactly halfway between two f32 values; the negation of that square
/// rounded to f32; `2^24`, above which adding 1 is lost; 1; an angle; 3; the smallest normal f32; and one half.
/// The first two are given as bit patterns, being exact values with more decimal digits than an f32 literal takes.
pub const INPUTS: [f32; 8] = [
    f32::from_bits(0x3f80_0800),
    f32::from_bits(0xbf80_1000),
    16_777_216.0_f32,
    1.0_f32,
    0.1_f32,
    3.0_f32,
    f32::MIN_POSITIVE,
    0.5_f32,
];

/// The interface a signature was measured through, where the probes ran as WGSL compute shaders on WebGPU.
pub const WEBGPU: &str = "webgpu";

/// The interface a signature was measured through, where the probes ran as GLSL fragment shaders on WebGL 2.
pub const WEBGL2: &str = "webgl2";

/// Each probe's name, the WGSL expression it evaluates, and the same expression in GLSL ES 3.00, in output order.
///
/// The first five and the last have an exact answer under strict evaluation and a second, equally legal one,
/// which [`CHOICES`] names. The rest carry an accuracy bound instead of an exact answer. GLSL ES 3.00 has no
/// `fma`, so that probe is one a WebGL 2 device is expected to refuse.
const PROBES: [(&str, &str, &str); PROBE_COUNT] = [
    (
        "multiply_add",
        "ulpBase * ulpBase + negatedSquare",
        "ulpBase * ulpBase + negatedSquare",
    ),
    (
        "fma",
        "fma(ulpBase, ulpBase, negatedSquare)",
        "fma(ulpBase, ulpBase, negatedSquare)",
    ),
    ("swamped_sum", "sum - big", "sum - big"),
    ("divide", "one / three", "one / three"),
    ("subnormal_product", "tiny * oneHalf", "tiny * oneHalf"),
    ("sqrt", "sqrt(three)", "sqrt(three)"),
    ("inverse_sqrt", "inverseSqrt(three)", "inversesqrt(three)"),
    ("sin", "sin(angle)", "sin(angle)"),
    ("cos", "cos(angle)", "cos(angle)"),
    ("tan", "tan(angle)", "tan(angle)"),
    ("exp", "exp(angle)", "exp(angle)"),
    ("log", "log(three)", "log(three)"),
    ("pow", "pow(three, angle)", "pow(three, angle)"),
    ("atan2", "atan2(angle, three)", "atan(angle, three)"),
    ("tanh", "tanh(angle)", "tanh(angle)"),
    (
        "dot",
        "dot(vec4<f32>(big, one, one, -big), vec4<f32>(one, one, one, one))",
        "dot(vec4(big, one, one, -big), vec4(one, one, one, one))",
    ),
];

/// The vertex shader every fragment probe is drawn with: one point covering the single pixel that is read back.
pub const VERTEX_SHADER: &str = "#version 300 es
void main() {
    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
    gl_PointSize = 1.0;
}
";

/// The part of every fragment probe shader that comes before its expression.
const FRAGMENT_HEAD: &str = "#version 300 es
precision highp float;
precision highp int;
uniform float probeIn[8];
out vec4 probeOut;

void main() {
    float ulpBase = probeIn[0];
    float negatedSquare = probeIn[1];
    float big = probeIn[2];
    float one = probeIn[3];
    float angle = probeIn[4];
    float three = probeIn[5];
    float tiny = probeIn[6];
    float oneHalf = probeIn[7];

    float sum = big;
    for (int turn = 0; turn < 4; turn++) {
        sum = sum + one;
    }

    uint bits = floatBitsToUint(";

/// The part of every fragment probe shader that comes after its expression.
///
/// It writes the result's four bytes, lowest first, as the red, green, blue and alpha of the one pixel drawn.
const FRAGMENT_TAIL: &str = ");
    probeOut = vec4(
        float(bits & 255u),
        float((bits >> 8) & 255u),
        float((bits >> 16) & 255u),
        float(bits >> 24)
    ) / 255.0;
}
";

/// The part of every compute probe shader that comes before its expression.
const SHADER_HEAD: &str = "
@group(0) @binding(0) var<storage, read>       probeIn  : array<f32, 8>;
@group(0) @binding(1) var<storage, read_write> probeOut : array<u32, 1>;

@compute @workgroup_size(1)
fn main() {
    let ulpBase = probeIn[0];
    let negatedSquare = probeIn[1];
    let big = probeIn[2];
    let one = probeIn[3];
    let angle = probeIn[4];
    let three = probeIn[5];
    let tiny = probeIn[6];
    let oneHalf = probeIn[7];

    var sum : f32 = big;
    for (var turn : u32 = 0u; turn < 4u; turn = turn + 1u) {
        sum = sum + one;
    }

    probeOut[0] = bitcast<u32>(";

/// The part of every compute probe shader that comes after its expression.
const SHADER_TAIL: &str = ");
}
";

/// For each probe with two legal exact answers: its name, then each answer's bits beside what giving it means.
const CHOICES: [(&str, [(u32, &str); 2]); 5] = [
    ("multiply_add", [(0, "rounded_twice"), (0x3380_0000, "fused")]),
    ("fma", [(0, "rounded_twice"), (0x3380_0000, "fused")]),
    ("swamped_sum", [(0, "in_order"), (0x4080_0000, "reassociated")]),
    ("subnormal_product", [(0x0040_0000, "kept"), (0, "flushed")]),
    ("dot", [(0, "in_order"), (0x4000_0000, "reassociated")]),
];

/// The interface each known GPU was measured through, what it answered in output order, and what that GPU is.
///
/// A label says where the answers were measured, not that no other GPU gives them: chips of one family that
/// share a driver may well answer alike, and only a measurement on each can tell them apart. The interface is
/// part of the key because one GPU answers differently through each: the shaders reach it by different compilers.
///
/// Each answer is the hex word a fingerprint shows for it, `????????` where the device refused the probe.
const KNOWN: [(&str, [&str; PROBE_COUNT], &str); 7] = [
    (
        WEBGL2,
        [
            "00000000", "????????", "40800000", "3eaaaaaa", "00000000", "3fddb3d7", "3f13cd3a", "3dcc7577", "3f7eb897",
            "3dcd7c45", "3f8d763e", "3f8c9f54", "3f8edd20", "3d087707", "3dcc1ebe", "3f800000",
        ],
        "AMD Radeon Graphics (0x15E7) through Direct3D 11 (measured on an HP EliteBook)",
    ),
    (
        WEBGL2,
        [
            "00000000", "????????", "00000000", "3eaaaaab", "00000000", "3fddb3d7", "3f13cd3a", "3dcc758b", "3f7eb898",
            "3dcd7c58", "3f8d763d", "3f8c9f54", "3f8edd1f", "3d087b9e", "3dcc1eba", "00000000",
        ],
        "Qualcomm Adreno 610 through OpenGL ES (measured on an Oppo phone)",
    ),
    (
        WEBGL2,
        [
            "33800000", "????????", "00000000", "3eaaaaab", "00000000", "3fddb3d7", "3f13cd3a", "3dcc75ec", "3f7eb8a9",
            "3dcd7cac", "3f8d763d", "3f8c9f54", "3f8edd1f", "3d087b99", "3dcc1ebc", "40000000",
        ],
        "Intel UHD Graphics 630 through Metal on macOS",
    ),
    (
        WEBGL2,
        [
            "33800000", "????????", "00000000", "3eaaaaab", "00400000", "3fddb3d7", "3f13cd3a", "3dcc7576", "3f7eb898",
            "3dcd7c43", "3f8d763e", "3f8c9f54", "3f8edd20", "3d087b99", "3dcc1ebe", "00000000",
        ],
        "ARM Mali-G76 through OpenGL ES (measured on a Samsung Galaxy S10)",
    ),
    (
        WEBGL2,
        [
            "33800000", "????????", "40800000", "3eaaaaab", "00000000", "3fddb3d7", "3f13cd3a", "3dcc756a", "3f7eb898",
            "3dcd7c38", "3f8d763d", "3f8c9f54", "3f8edd1f", "3d087ae6", "3dcc1eb6", "3f800000",
        ],
        "Intel Iris Xe (Alder Lake GT2) through Mesa on Linux",
    ),
    (
        WEBGPU,
        [
            "33800000", "33800000", "40800000", "3eaaaaab", "00000000", "3fddb3d7", "3f13cd3a", "3dcc7576", "3f7eb899",
            "3dcd7c43", "3f8d763e", "3f8c9f54", "3f8edd1f", "3d087b99", "3dcc1ebd", "00000000",
        ],
        "Apple silicon GPU through Metal (measured on an Apple M5 Pro)",
    ),
    (
        WEBGL2,
        [
            "33800000", "????????", "40800000", "3eaaaaab", "00000000", "3fddb3d7", "3f13cd3a", "3dcc7576", "3f7eb899",
            "3dcd7c43", "3f8d763e", "3f8c9f54", "3f8edd1f", "3d087b99", "3dcc1ebc", "00000000",
        ],
        "Apple silicon GPU through Metal (measured on an Apple M5 Pro)",
    ),
];

/// What a device is classed as when no known GPU answers the probe set as it did.
pub const UNRECOGNISED: &str = "unrecognised";

/// Each probe's name beside the WGSL compute shader that evaluates it, in output order.
///
/// Every shader reads [`INPUTS`] from an 8-element `f32` storage buffer at binding 0 and writes its result's bits
/// to a 1-element `u32` storage buffer at binding 1, from an entry point named `main`.
#[must_use]
pub fn shaders() -> Vec<(&'static str, String)> {
    PROBES
        .iter()
        .map(|probe| (probe.0, [SHADER_HEAD, probe.1, SHADER_TAIL].concat()))
        .collect()
}

/// Each probe's name beside the GLSL ES 3.00 fragment shader that evaluates it, in output order.
///
/// Every shader reads [`INPUTS`] from the 8-element `float` uniform array `probeIn` and writes its result's four
/// bytes, lowest first, to the red, green, blue and alpha of its one output, so it is drawn with
/// [`VERTEX_SHADER`] into an 8-bit RGBA target and the pixel read back.
#[must_use]
pub fn fragment_shaders() -> Vec<(&'static str, String)> {
    PROBES
        .iter()
        .map(|probe| (probe.0, [FRAGMENT_HEAD, probe.2, FRAGMENT_TAIL].concat()))
        .collect()
}

/// `value` rounded to the nearest f32.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "rounding an f64 to the nearest f32 is the conversion wanted, and std offers no other spelling of it"
)]
const fn nearest(value: f64) -> f32 {
    value as f32
}

/// The bits of each probe's correctly rounded answer, in output order.
///
/// The exact probes are evaluated as written, one f32 operation at a time. The mathematical functions are
/// evaluated in f64 and rounded once to f32, which is the correctly rounded f32 answer unless the true value
/// lies within an f64 rounding error of halfway between two f32 values -- and none of these inputs' does.
#[must_use]
#[expect(
    clippy::float_arithmetic,
    clippy::suboptimal_flops,
    reason = "the probes are float arithmetic, and the unfused multiply-add is one of the answers being told apart"
)]
pub fn reference() -> [u32; PROBE_COUNT] {
    let [ulp_base, negated_square, big, one, angle, three, tiny, half] = INPUTS;
    let wide_angle = f64::from(angle);
    let wide_three = f64::from(three);
    let mut sum = big;
    for _step in 0_u32..4_u32 {
        sum += one;
    }
    [
        (ulp_base * ulp_base + negated_square).to_bits(),
        ulp_base.mul_add(ulp_base, negated_square).to_bits(),
        (sum - big).to_bits(),
        (one / three).to_bits(),
        (tiny * half).to_bits(),
        three.sqrt().to_bits(),
        nearest(wide_three.sqrt().recip()).to_bits(),
        nearest(wide_angle.sin()).to_bits(),
        nearest(wide_angle.cos()).to_bits(),
        nearest(wide_angle.tan()).to_bits(),
        nearest(wide_angle.exp()).to_bits(),
        nearest(wide_three.ln()).to_bits(),
        nearest(wide_three.powf(wide_angle)).to_bits(),
        nearest(wide_angle.atan2(wide_three)).to_bits(),
        nearest(wide_angle.tanh()).to_bits(),
        [big * one, one * one, one * one, -big * one]
            .into_iter()
            .fold(0.0_f32, |total, product| total + product)
            .to_bits(),
    ]
}

/// What a device did with one probe.
///
/// A refusal is an outcome to record, not a failure of the probe set: which probes a device will not run is
/// part of what tells it from another.
#[non_exhaustive]
pub enum Answer {
    /// The bits of the result the device computed.
    Bits(u32),
    /// The device gave no result, for the reason held: a shader it would not compile, or a read that failed.
    Refused(String),
}

/// What one device returned for the probe set through one interface.
pub struct Signature {
    /// The interface the probes ran through: [`WEBGPU`] or [`WEBGL2`].
    interface: &'static str,
    /// What the device did with each probe, in output order.
    observed: Vec<Answer>,
}

impl Signature {
    /// Read what a device did with each probe, in output order, as measured through `interface`.
    #[must_use]
    pub const fn new(interface: &'static str, observed: Vec<Answer>) -> Self {
        Self { interface, observed }
    }

    /// Every result as a hex word, `????????` where the device gave none, in output order.
    fn words(&self) -> Vec<String> {
        self.observed
            .iter()
            .map(|answer| match answer {
                Answer::Bits(bits) => format!("{bits:08x}"),
                Answer::Refused(_) => "????????".to_owned(),
            })
            .collect()
    }

    /// How many probes the device answered.
    #[must_use]
    pub fn completed(&self) -> usize {
        self.observed
            .iter()
            .filter(|answer| matches!(answer, Answer::Bits(_)))
            .count()
    }

    /// How many probes the device answered with bits other than the correctly rounded ones.
    #[must_use]
    pub fn deviations(&self) -> usize {
        self.observed
            .iter()
            .zip(reference())
            .filter(|pair| matches!(pair.0, Answer::Bits(bits) if *bits != pair.1))
            .count()
    }

    /// Every result as a hex word, `????????` where the device gave none, joined by `-`.
    ///
    /// Two devices have the same fingerprint exactly when they answered every probe the same way.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        self.words().join("-")
    }

    /// The known GPU that answers the probe set as this device did, or [`UNRECOGNISED`].
    ///
    /// A match is through the same interface and on every probe, refusals included.
    #[must_use]
    pub fn class(&self) -> &'static str {
        let words = self.words();
        KNOWN
            .iter()
            .find(|known| known.0 == self.interface && words == known.1)
            .map_or(UNRECOGNISED, |known| known.2)
    }

    /// Which legal answer the device gave to each probe that has two, as `name=choice` joined by spaces.
    ///
    /// A probe the device did not answer reads `failed`, and an answer that is neither of the two reads `other`.
    #[must_use]
    pub fn traits(&self) -> String {
        CHOICES
            .iter()
            .map(|choice| {
                let chosen = PROBES
                    .iter()
                    .zip(&self.observed)
                    .find(|pair| pair.0.0 == choice.0)
                    .map_or("failed", |pair| match pair.1 {
                        Answer::Bits(bits) => choice
                            .1
                            .iter()
                            .find(|legal| legal.0 == *bits)
                            .map_or("other", |legal| legal.1),
                        Answer::Refused(_) => "failed",
                    });
                format!("{}={chosen}", choice.0)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// One line per probe, saying what the device did with it.
    ///
    /// An answered probe gives its name, the device's bits, the correctly rounded bits, and the distance between
    /// them in units in the last place. A refused one gives its name and the reason.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        PROBES
            .iter()
            .zip(self.observed.iter().zip(reference()))
            .map(|(probe, (answer, correct))| match answer {
                Answer::Bits(bits) => format!(
                    "{} gpu={bits:08x} correct={correct:08x} ulps={:+}",
                    probe.0,
                    i64::from(*bits).saturating_sub(i64::from(correct))
                ),
                Answer::Refused(reason) => format!("{} failed: {reason}", probe.0),
            })
            .collect()
    }
}
