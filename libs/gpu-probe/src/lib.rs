//! Numeric probes that tell one GPU's floating-point behaviour from another's.
//!
//! A shading language leaves an implementation room on most floating-point operations: whether a multiply and an add
//! are fused, the order a sum is taken in, whether a subnormal survives, and how closely each mathematical function is
//! rounded. Two GPUs can therefore give different bits for the same expression and both be correct. This crate holds
//! a fixed set of such expressions, the WGSL that evaluates each, and the correctly rounded answer, and reads what a
//! device returned as a [`Signature`]: which choices it made, how far each result sits from the correctly rounded one,
//! and which known GPU, if any, answers the same way.
//!
//! It drives no GPU itself, so the same probes serve a browser's WebGPU, a native host's, and a caller with none. Each
//! probe is a shader of its own: a device that refuses one expression still answers the rest, and a signature with gaps
//! in it identifies a device better than no signature.
//!
//! Two more probe sets reach the GPU where no shader compiler decides the answer. [`CLEARS`] clears a target to a
//! value with no shader at all, so the driver or the output stage converts the value to the target's format. The fixed-
//! function set -- [`BLENDS`], [`FILTERS`] and [`RASTERS`] -- draws with a shader that only hands the blend unit, the
//! texture unit or the rasterizer a constant, so the arithmetic read back is that stage's.
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

#[cfg(feature = "registry")]
pub use platforms::OS;

mod canvas;
mod clear;
mod fixed;
mod format;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "registry")]
mod registry;
#[cfg(feature = "registry")]
mod sighting;

pub use self::canvas::{
    Alteration, CANVAS, CANVAS_BROWSERLEAKS, KNOWN_BLOCK, KNOWN_SIZE, ReadLength, alteration, known_colour,
    known_scene, pixel_hash,
};
pub use self::clear::{CLEAR_COUNT, CLEARS, Clear, clear_reference};
pub use self::fixed::{
    BLENDS, Blend, FILTERS, FIXED_FUNCTION_COUNT, Filter, Operation, RASTER_FORMAT, RASTER_WIDTH, RASTERS, Raster,
    Side, coverage, fixed_function_reference,
};
pub use self::format::Format;
#[cfg(feature = "registry")]
pub use self::registry::every_sighting;
#[cfg(feature = "registry")]
pub use self::sighting::{Backend, Browser, GpuVendor, RowError, Sighting, Window};

/// How many expressions the probe set evaluates.
pub const PROBE_COUNT: usize = 16;

/// The probes' inputs, handed to each shader in a storage buffer so that no expression is folded at compile time.
///
/// In order: `1 + 2^-12`, whose square falls exactly halfway between two f32 values; the negation of that square
/// rounded to f32; `2^24`, above which adding 1 is lost; 1; an angle; 3; the smallest normal f32; and one half. The
/// first two are given as bit patterns, being exact values with more decimal digits than an f32 literal takes.
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
///
/// That is WebGPU whoever implements it: a browser, Deno, or wgpu called with no browser at all. One implementation
/// answers the same way in each; a native wgpu device gives the signature Firefox gives on the same GPU.
pub const WEBGPU: &str = "webgpu";

/// [`WEBGPU`] where every probe ran in the one shader [`combined_shader`] gives, not in a shader of its own.
pub const WEBGPU_COMBINED: &str = "webgpu-combined";

/// The interface a signature was measured through, where the probes ran as GLSL fragment shaders on WebGL 2.
pub const WEBGL2: &str = "webgl2";

/// The interface a signature was measured through, where the CPU evaluated the probes as `f32` arithmetic.
///
/// The mathematical functions are then the answers of the platform's own math library.
pub const HOST: &str = "host";

/// The interface a signature was measured through, where the probes ran as ONNX operators behind `wasi:nn`.
pub const WASI_NN: &str = "wasi-nn";

/// The interface a signature of [`CLEARS`] was measured through, each a render pass clearing a target on WebGPU.
pub const WEBGPU_CLEAR: &str = "webgpu-clear";

/// The interface a signature of the fixed-function probes was measured through, each drawn on WebGPU.
pub const WEBGPU_FIXED_FUNCTION: &str = "webgpu-fixed-function";

/// The interface a signature of [`CLEARS`] was measured through, each a `clearBufferfv` of a target on WebGL 2.
pub const WEBGL2_CLEAR: &str = "webgl2-clear";

/// The interface a signature of the fixed-function probes was measured through, each drawn on WebGL 2.
pub const WEBGL2_FIXED_FUNCTION: &str = "webgl2-fixed-function";

/// Each probe's name, the WGSL expression it evaluates, and the same expression in GLSL ES 3.00, in output order.
///
/// The first five and the last have an exact answer under strict evaluation and at least one other, equally legal one,
/// which [`CHOICES`] names. The rest carry an accuracy bound instead of an exact answer. GLSL ES 3.00 has no `fma`, so
/// that probe is one a WebGL 2 device is expected to refuse.
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

/// Where in output order the probes of mathematical functions start, which run up to the last probe.
const FIRST_FUNCTION: usize = 6;

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

/// The part of every compute probe shader that comes before the number of words it writes.
const SHADER_OPEN: &str = "
@group(0) @binding(0) var<storage, read>       probeIn  : array<f32, 8>;
@group(0) @binding(1) var<storage, read_write> probeOut : array<u32, ";

/// The part of every compute probe shader between the number of words it writes and its expressions.
const SHADER_HEAD: &str = ">;

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

";

/// For each probe with more than one legal exact answer: its name, then each answer's bits beside what giving it means.
///
/// A sum the compiler may regroup has one legal answer per grouping that gives a distinct result. `swamped_sum` is
/// `2^24` plus four ones: added one at a time each one is lost, the ones summed first survive as 4, and a loop unrolled
/// to add its last two as a pair keeps only those two. `dot` is `2^24 + 1 + 1 - 2^24`: in order both ones are lost, the
/// two `2^24` cancelling first keeps both, and summed as a pairwise tree, `(2^24 + 1) + (1 - 2^24)`, keeps one.
const CHOICES: [(&str, &[(u32, &str)]); 5] = [
    ("multiply_add", &[(0, "rounded_twice"), (0x3380_0000, "fused")]),
    ("fma", &[(0, "rounded_twice"), (0x3380_0000, "fused")]),
    (
        "swamped_sum",
        &[(0, "in_order"), (0x4080_0000, "reassociated"), (0x4000_0000, "paired")],
    ),
    ("subnormal_product", &[(0x0040_0000, "kept"), (0, "flushed")]),
    (
        "dot",
        &[
            (0, "in_order"),
            (0x4000_0000, "reassociated"),
            (0x3f80_0000, "pairwise"),
        ],
    ),
];

/// The interface each known GPU was measured through, what it answered in output order, and what that GPU is.
///
/// A label says where the answers were measured, not that no other GPU gives them: chips of one family that share a
/// driver may well answer alike, and only a measurement on each can tell them apart. The interface is part of the key
/// because one GPU answers differently through each: the shaders reach it by different compilers.
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

/// The answers a probe set names: a probe's name, then each answer beside what giving it means.
type Choices = [(&'static str, &'static [(u32, &'static str)])];

/// A probe set: each probe's name and correct answer, in output order, and the answers it names.
struct ProbeSet {
    /// Each probe's name, in output order.
    names: Vec<&'static str>,
    /// Each probe's correct answer, in output order.
    reference: Vec<u32>,
    /// The answers the set names.
    choices: &'static Choices,
}

impl ProbeSet {
    /// The probe set a signature measured through `interface` answers.
    fn measured_through(interface: &str) -> Self {
        match interface {
            WEBGPU_CLEAR | WEBGL2_CLEAR => Self {
                names: CLEARS.iter().map(|probe| probe.name).collect(),
                reference: clear_reference(),
                choices: &clear::CHOICES,
            },
            WEBGPU_FIXED_FUNCTION | WEBGL2_FIXED_FUNCTION => Self {
                names: fixed::names(),
                reference: fixed_function_reference(),
                choices: &fixed::CHOICES,
            },
            _ => Self {
                names: PROBES.iter().map(|probe| probe.0).collect(),
                reference: reference().to_vec(),
                choices: &CHOICES,
            },
        }
    }
}

/// Each probe's name beside the WGSL compute shader that evaluates it, in output order.
///
/// Every shader reads [`INPUTS`] from an 8-element `f32` storage buffer at binding 0 and writes its result's bits to a
/// 1-element `u32` storage buffer at binding 1, from an entry point named `main`.
#[must_use]
pub fn shaders() -> Vec<(&'static str, String)> {
    PROBES
        .iter()
        .map(|probe| (probe.0, compute_shader(&[probe.1])))
        .collect()
}

/// The one WGSL compute shader that evaluates every probe, writing each result's bits in output order.
///
/// It is bound as each of [`shaders`] is, except that its buffer at binding 1 holds [`PROBE_COUNT`] words. A compiler
/// may treat an expression differently when it sits beside the others, so a device can answer this shader and the
/// separate ones differently, and a signature taken with it is filed under [`WEBGPU_COMBINED`]. A device that refuses
/// one expression refuses the whole of it.
#[must_use]
pub fn combined_shader() -> String {
    let expressions: Vec<&str> = PROBES.iter().map(|probe| probe.1).collect();
    compute_shader(&expressions)
}

/// A WGSL compute shader that writes the bits of each of `expressions` to a word of its own, in order.
fn compute_shader(expressions: &[&str]) -> String {
    let writes: Vec<String> = expressions
        .iter()
        .enumerate()
        .map(|(word, expression)| format!("    probeOut[{word}] = bitcast<u32>({expression});\n"))
        .collect();
    format!("{SHADER_OPEN}{}{SHADER_HEAD}{}}}\n", expressions.len(), writes.concat())
}

/// Each probe's name beside the GLSL ES 3.00 fragment shader that evaluates it, in output order.
///
/// Every shader reads [`INPUTS`] from the 8-element `float` uniform array `probeIn` and writes its result's four bytes,
/// lowest first, to the red, green, blue and alpha of its one output, so it is drawn with [`VERTEX_SHADER`] into an 8-
/// bit RGBA target and the pixel read back.
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
pub(crate) const fn nearest(value: f64) -> f32 {
    value as f32
}

/// The bits of each probe's correctly rounded answer, in output order.
///
/// The exact probes are evaluated as written, one f32 operation at a time. The mathematical functions are evaluated
/// in f64 and rounded once to f32, which is the correctly rounded f32 answer unless the true value lies within an f64
/// rounding error of halfway between two f32 values -- and none of these inputs' does.
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

/// The bits of each probe as the CPU this runs on answers it, in output order.
///
/// The arithmetic is `f32` operations taken one at a time, so the exact probes give their strict answers, as they do
/// in [`reference()`]. The mathematical functions are the `f32` ones of the platform's math library, which are not
/// correctly rounded and differ from one library to the next. The inputs pass through `black_box` so that the functions
/// are evaluated when this is called, on this machine, and not folded by the compiler on the one that built it.
#[must_use]
pub fn host() -> [u32; PROBE_COUNT] {
    let [_, _, _, _, angle, three, _, _] = core::hint::black_box(INPUTS);
    let functions = [
        three.sqrt().recip(),
        angle.sin(),
        angle.cos(),
        angle.tan(),
        angle.exp(),
        three.ln(),
        three.powf(angle),
        angle.atan2(three),
        angle.tanh(),
    ];
    let mut bits = reference();
    for (answer, function) in bits.iter_mut().skip(FIRST_FUNCTION).zip(functions) {
        *answer = function.to_bits();
    }
    bits
}

/// What a device did with one probe.
///
/// A refusal is an outcome to record, not a failure of the probe set: which probes a device will not run is part of
/// what tells it from another.
#[non_exhaustive]
pub enum Answer {
    /// The bits of the result the device computed.
    Bits(u32),
    /// The device gave no result, for the reason held: a shader it would not compile, or a read that failed.
    Refused(String),
}

/// What one device returned for the probe set through one interface.
pub struct Signature {
    /// The interface the probes ran through: [`WEBGPU`], [`WEBGPU_COMBINED`], [`WEBGL2`], [`HOST`] or [`WASI_NN`].
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

    /// How many probes the device answered with bits other than the correct ones.
    #[must_use]
    pub fn deviations(&self) -> usize {
        self.observed
            .iter()
            .zip(ProbeSet::measured_through(self.interface).reference)
            .filter(|pair| matches!(pair.0, Answer::Bits(bits) if *bits != pair.1))
            .count()
    }

    /// Each probe's name beside its result as a hex word, `????????` where the device gave none, in output order.
    ///
    /// These are an entry's `results` in the registry.
    #[must_use]
    pub fn results(&self) -> Vec<(&'static str, String)> {
        ProbeSet::measured_through(self.interface)
            .names
            .into_iter()
            .zip(self.words())
            .collect()
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

    /// The label of every registry entry holding the answers this device gave, through the same interface.
    ///
    /// A label names every device, browser and backend the registry has seen the signature on. No label means the
    /// registry has not seen it, and more than one that it is filed more than once.
    ///
    /// # Stability
    ///
    /// Not a stable part of this crate's API. The registry is data kept for checking and looking into what a device
    /// answered, and its entries, how they are structured and what a label says all change without that being a
    /// breaking change. Nothing a user ships should depend on the text returned.
    #[cfg(feature = "registry")]
    #[must_use]
    pub fn identify(&self) -> Vec<&'static str> {
        registry::labels(self.interface, &self.words())
    }

    /// Every place the registry has seen the answers this device gave, through the same interface.
    ///
    /// Each is one row of an entry [`identify`](Self::identify) would name: the browser, operating system, CPU
    /// architecture, GPU vendor, graphics API and kind of window the signature was seen with together. Only what a row
    /// names from fixed sets of values is given; its free text stays in the registry.
    ///
    /// # Stability
    ///
    /// The rows the registry holds change as signatures are seen in new places, without that being a breaking change.
    #[cfg(feature = "registry")]
    pub fn seen(&self) -> Result<Vec<Sighting>, RowError> {
        registry::seen(self.interface, &self.words())
    }

    /// Which named answer the device gave to each probe of its set that has more than one.
    ///
    /// Each is `name=choice`, joined by spaces. A probe the device did not answer reads `failed`, and an answer that is
    /// none of those named reads `other`.
    #[must_use]
    pub fn traits(&self) -> String {
        let set = ProbeSet::measured_through(self.interface);
        set.choices
            .iter()
            .map(|choice| {
                let chosen = set
                    .names
                    .iter()
                    .zip(&self.observed)
                    .find(|pair| *pair.0 == choice.0)
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
    /// An answered probe gives its name, the device's bits, the correct bits, and the distance between them in units
    /// in the last place -- of the f32, or of the format a clear or a blend wrote. A refused one gives its name and
    /// the reason.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        let set = ProbeSet::measured_through(self.interface);
        set.names
            .iter()
            .zip(self.observed.iter().zip(set.reference))
            .map(|(name, (answer, correct))| match answer {
                Answer::Bits(bits) => format!(
                    "{name} gpu={bits:08x} correct={correct:08x} ulps={:+}",
                    i64::from(*bits).saturating_sub(i64::from(correct))
                ),
                Answer::Refused(reason) => format!("{name} failed: {reason}"),
            })
            .collect()
    }
}
