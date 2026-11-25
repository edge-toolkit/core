//! Put hand-written PTX probes to every CUDA device through NVIDIA's driver API, and print what each answers.
//!
//! Each probe is a PTX module of its own, naming one instruction and the oldest architecture that has it, so a device
//! without it refuses that probe alone. The driver compiles every module for the device it is loaded on, whatever
//! architecture the module names, so the answers are always that device's own; the last table shows as much. An
//! elementwise probe answers 32 inputs, one per thread, each in a 64-bit slot of which it uses the low bytes, and is
//! scored in ulps against the host's correctly rounded answer. A tensor-core probe runs one warp-wide `mma` per case.

use core::ffi::c_void;
use core::fmt::Write as _;
use std::error::Error;
use std::io::{StdoutLock, Write as _};
use std::sync::Arc;

use cuda_core::{Context, DeviceBuffer, DeviceCopy, DriverError, IntoResult as _, Stream, launch_kernel_on_stream};
use half::f16;

/// The width of the value an elementwise probe reads and writes.
#[derive(Clone, Copy)]
enum Kind {
    F16,
    F32,
    F64,
}

impl Kind {
    /// The bits `value` has at this width, rounded to it.
    fn encode(self, value: f64) -> u64 {
        match self {
            Self::F16 => u64::from(f16::from_f64(value).to_bits()),
            Self::F32 => u64::from(narrow(value).to_bits()),
            Self::F64 => value.to_bits(),
        }
    }

    /// The value `bits` holds at this width; not a number where they are wider than it.
    fn decode(self, bits: u64) -> f64 {
        match self {
            Self::F16 => u16::try_from(bits).map_or(f64::NAN, |half| f16::from_bits(half).to_f64()),
            Self::F32 => u32::try_from(bits).map_or(f64::NAN, |single| f64::from(f32::from_bits(single))),
            Self::F64 => f64::from_bits(bits),
        }
    }

    /// The sign bit at this width.
    const fn sign(self) -> u64 {
        match self {
            Self::F16 => 0x8000,
            Self::F32 => 0x8000_0000,
            Self::F64 => 0x8000_0000_0000_0000,
        }
    }
}

/// One elementwise probe: `body`, an instruction taking register 1 to register 2, beside the host's `reference`.
#[derive(Clone, Copy)]
struct Probe {
    name: &'static str,
    arch: u32,
    ptx_version: &'static str,
    kind: Kind,
    body: &'static str,
    reference: fn(f64) -> f64,
}

/// The probe `name`, run as `body` at `sm_<arch>` and PTX ISA `ptx_version`, and answered on the host by `reference`.
const fn op(
    name: &'static str,
    arch: u32,
    ptx_version: &'static str,
    kind: Kind,
    body: &'static str,
    reference: fn(f64) -> f64,
) -> Probe {
    Probe {
        name,
        arch,
        ptx_version,
        kind,
        body,
        reference,
    }
}

/// Every elementwise probe: the approximate instructions, and two correctly rounded ones that check the harness.
const OPS: [Probe; 16] = [
    op(
        "sin.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "sin.approx.f32 %f2, %f1;",
        f64::sin,
    ),
    op(
        "cos.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "cos.approx.f32 %f2, %f1;",
        f64::cos,
    ),
    op(
        "ex2.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "ex2.approx.f32 %f2, %f1;",
        f64::exp2,
    ),
    op(
        "lg2.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "lg2.approx.f32 %f2, %f1;",
        f64::log2,
    ),
    op(
        "rsqrt.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "rsqrt.approx.f32 %f2, %f1;",
        |value| value.sqrt().recip(),
    ),
    op(
        "rcp.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "rcp.approx.f32 %f2, %f1;",
        f64::recip,
    ),
    op(
        "sqrt.approx.f32",
        52,
        "6.0",
        Kind::F32,
        "sqrt.approx.f32 %f2, %f1;",
        f64::sqrt,
    ),
    op(
        "div.approx.f32 x/3",
        52,
        "6.0",
        Kind::F32,
        "div.approx.f32 %f2, %f1, 0f40400000;",
        |value| value / 3.0_f64,
    ),
    op(
        "div.full.f32 x/3",
        52,
        "6.0",
        Kind::F32,
        "div.full.f32 %f2, %f1, 0f40400000;",
        |value| value / 3.0_f64,
    ),
    op(
        "div.rn.f32 x/3 (exact)",
        52,
        "6.0",
        Kind::F32,
        "div.rn.f32 %f2, %f1, 0f40400000;",
        |value| value / 3.0_f64,
    ),
    op(
        "sqrt.rn.f32 (exact)",
        52,
        "6.0",
        Kind::F32,
        "sqrt.rn.f32 %f2, %f1;",
        f64::sqrt,
    ),
    op(
        "tanh.approx.f32",
        75,
        "7.0",
        Kind::F32,
        "tanh.approx.f32 %f2, %f1;",
        f64::tanh,
    ),
    op(
        "rcp.approx.ftz.f64",
        52,
        "6.0",
        Kind::F64,
        "rcp.approx.ftz.f64 %fd2, %fd1;",
        f64::recip,
    ),
    op(
        "rsqrt.approx.f64",
        52,
        "6.0",
        Kind::F64,
        "rsqrt.approx.f64 %fd2, %fd1;",
        |value| value.sqrt().recip(),
    ),
    op(
        "tanh.approx.f16",
        75,
        "7.0",
        Kind::F16,
        "tanh.approx.f16 %h2, %h1;",
        f64::tanh,
    ),
    op(
        "ex2.approx.f16",
        75,
        "7.0",
        Kind::F16,
        "ex2.approx.f16 %h2, %h1;",
        f64::exp2,
    ),
];

/// The 32 inputs every elementwise probe answers, as f32 bits: `0.11 + 0.37 * i`, all positive so each is defined.
const INPUTS: [u32; 32] = [
    0x3de1_47ae,
    0x3ef5_c28f,
    0x3f59_999a,
    0x3f9c_28f6,
    0x3fcb_851f,
    0x3ffa_e148,
    0x4015_1eb8,
    0x402c_cccd,
    0x4044_7ae1,
    0x405c_28f6,
    0x4073_d70a,
    0x4085_c28f,
    0x4091_999a,
    0x409d_70a4,
    0x40a9_47ae,
    0x40b5_1eb8,
    0x40c0_f5c3,
    0x40cc_cccd,
    0x40d8_a3d7,
    0x40e4_7ae1,
    0x40f0_51ec,
    0x40fc_28f6,
    0x4104_0000,
    0x4109_eb85,
    0x410f_d70a,
    0x4115_c28f,
    0x411b_ae14,
    0x4121_999a,
    0x4127_851f,
    0x412d_70a4,
    0x4133_5c29,
    0x4139_47ae,
];

/// Each PTX target the last table names, from Maxwell to Blackwell, beside the oldest PTX ISA that can name it.
const TARGETS: [(u32, &str); 11] = [
    (52, "6.0"),
    (61, "6.0"),
    (70, "6.0"),
    (75, "7.0"),
    (80, "7.0"),
    (86, "7.1"),
    (89, "7.8"),
    (90, "7.8"),
    (100, "8.6"),
    (120, "8.7"),
    (121, "8.8"),
];

/// The operand and accumulator types of a tensor-core probe.
#[derive(Clone, Copy)]
enum Mma {
    /// m16n8k16, f16 inputs into an f32 accumulator.
    F16,
    /// m16n8k16, bf16 inputs into an f32 accumulator.
    Bf16,
    /// m16n8k8, tf32 inputs (passed as raw f32 bits) into an f32 accumulator.
    Tf32,
    /// m16n8k16, f16 inputs into an f16 accumulator.
    F16Acc,
}

impl Mma {
    const ALL: [Self; 4] = [Self::F16, Self::Bf16, Self::Tf32, Self::F16Acc];

    const fn name(self) -> &'static str {
        match self {
            Self::F16 => "f16->f32 m16n8k16",
            Self::Bf16 => "bf16->f32 m16n8k16",
            Self::Tf32 => "tf32->f32 m16n8k8",
            Self::F16Acc => "f16->f16 m16n8k16",
        }
    }

    const fn depth(self) -> usize {
        match self {
            Self::Tf32 => 8,
            Self::F16 | Self::Bf16 | Self::F16Acc => 16,
        }
    }

    /// The value the host takes `value` to be once it is put in this probe's input format.
    fn quantize(self, value: f32) -> f64 {
        match self {
            Self::F16 | Self::F16Acc => f16::from_f32(value).to_f64(),
            Self::Bf16 => half::bf16::from_f32(value).to_f64(),
            Self::Tf32 => f64::from(value),
        }
    }

    /// The input word bits of one operand element.
    fn element(self, value: f32) -> u32 {
        match self {
            Self::F16 | Self::F16Acc => u32::from(f16::from_f32(value).to_bits()),
            Self::Bf16 => u32::from(half::bf16::from_f32(value).to_bits()),
            Self::Tf32 => value.to_bits(),
        }
    }

    /// The accumulator word.
    fn accumulator(self, value: f32) -> u32 {
        match self {
            Self::F16Acc => {
                let half = u32::from(f16::from_f32(value).to_bits());
                half | half.rotate_left(16)
            }
            Self::F16 | Self::Bf16 | Self::Tf32 => value.to_bits(),
        }
    }

    /// The answer's bits out of the word lane 0 wrote.
    const fn answer(self, word: u32) -> u32 {
        match self {
            Self::F16Acc => word & 0xffff,
            Self::F16 | Self::Bf16 | Self::Tf32 => word,
        }
    }

    /// `exact` rounded to nearest at the accumulator's width.
    fn nearest(self, exact: f64) -> u32 {
        match self {
            Self::F16Acc => u32::from(f16::from_f64(exact).to_bits()),
            Self::F16 | Self::Bf16 | Self::Tf32 => narrow(exact).to_bits(),
        }
    }

    /// One warp-wide `mma` with every row of A and column of B the same.
    ///
    /// The input is 8 words of A's k elements, 8 of B's, then the accumulator; lane 0 writes one element of the
    /// result, which every element equals.
    fn entry(self) -> String {
        let (registers, load_c, mma, store) = match self {
            Self::F16 | Self::Bf16 | Self::Tf32 => {
                let (shape, input) = match self {
                    Self::Bf16 => ("m16n8k16", "bf16.bf16"),
                    Self::Tf32 => ("m16n8k8", "tf32.tf32"),
                    Self::F16 | Self::F16Acc => ("m16n8k16", "f16.f16"),
                };
                (
                    ".reg .f32 %f<6>;".to_owned(),
                    "ld.global.f32 %f1, [%rd1+64];",
                    format!(
                        "mma.sync.aligned.{shape}.row.col.f32.{input}.f32 {{%f2,%f3,%f4,%f5}}, {{%r3,%r3,%r4,%r4}}, \
                         {{%r5,%r6}}, {{%f1,%f1,%f1,%f1}};"
                    ),
                    "st.global.f32 [%rd2], %f2;",
                )
            }
            Self::F16Acc => (
                ".reg .b32 %q<4>;".to_owned(),
                "ld.global.b32 %q1, [%rd1+64];",
                "mma.sync.aligned.m16n8k16.row.col.f16.f16.f16.f16 {%q2,%q3}, {%r3,%r3,%r4,%r4}, {%r5,%r6}, {%q1,%q1};"
                    .to_owned(),
                "st.global.b32 [%rd2], %q2;",
            ),
        };
        format!(
            ".visible .entry probe(.param .u64 pin, .param .u64 pout)
{{
  .reg .pred %p;
  .reg .b32 %r<8>;
  .reg .b64 %rd<5>;
  {registers}
  ld.param.u64 %rd1, [pin];
  ld.param.u64 %rd2, [pout];
  cvta.to.global.u64 %rd1, %rd1;
  cvta.to.global.u64 %rd2, %rd2;
  mov.u32 %r1, %tid.x;
  and.b32 %r2, %r1, 3;
  mul.wide.u32 %rd3, %r2, 4;
  add.s64 %rd4, %rd1, %rd3;
  ld.global.b32 %r3, [%rd4];
  ld.global.b32 %r4, [%rd4+16];
  ld.global.b32 %r5, [%rd4+32];
  ld.global.b32 %r6, [%rd4+48];
  {load_c}
  {mma}
  setp.eq.u32 %p, %r1, 0;
  @%p {store}
  ret;
}}
"
        )
    }
}

/// A tensor-core case: the products `left * right` that are not zero, and the accumulator they are added to.
struct MmaCase {
    name: &'static str,
    products: Vec<(f32, f32)>,
    accumulator: f32,
}

/// Every tensor-core case, each chosen so that how the sum is aligned, truncated or rounded shows in its answer.
fn mma_cases() -> Vec<MmaCase> {
    let pow2 = |exponent: i32| 2.0_f32.powi(exponent);
    let tiny = pow2(-12_i32);
    // 1.5 * 2^-12 and its negation, so that each one's product with 2^-12 is three quarters of an ulp of 1.
    let tiny_and_a_half = f32::from_bits(0x39c0_0000);
    let less_tiny_and_a_half = f32::from_bits(0xb9c0_0000);
    let case = |name, products, accumulator| MmaCase {
        name,
        products,
        accumulator,
    };
    vec![
        case("c=1 + 16 x 2^-24", vec![(tiny, tiny); 16], 1.0),
        case(
            "1 + 15 x 2^-24",
            [vec![(1.0, 1.0)], vec![(tiny, tiny); 15]].concat(),
            0.0,
        ),
        case(
            "1 - 1 + 2^-20",
            vec![(1.0, 1.0), (-1.0, 1.0), (pow2(-10_i32), pow2(-10_i32))],
            0.0,
        ),
        case(
            "c=-1 + 1 + 2^-30",
            vec![(1.0, 1.0), (pow2(-15_i32), pow2(-15_i32))],
            -1.0,
        ),
        case("c=1 + 0.75 ulp", vec![(tiny_and_a_half, tiny)], 1.0),
        case("c=-1 - 0.75 ulp", vec![(less_tiny_and_a_half, tiny)], -1.0),
        case(
            "c=1 + 0.5 ulp + 0.125 ulp",
            vec![(tiny, tiny), (tiny, pow2(-14_i32))],
            1.0,
        ),
        case("16 x 2^16 + c=1", vec![(256.0, 256.0); 16], 1.0),
        case(
            "2^30 - 2^30 + 1",
            vec![(32768.0, 32768.0), (-32768.0, 32768.0), (1.0, 1.0)],
            0.0,
        ),
        case("16 x subnormal^2", vec![(pow2(-24_i32), pow2(-24_i32)); 16], 0.0),
        case(
            "3 x 2^-24 + 1 (late big)",
            vec![(tiny, tiny), (tiny, tiny), (tiny, tiny), (1.0, 1.0)],
            0.0,
        ),
        // How many bits below the accumulator's ulp survive alignment: 2^-25, 2^-26 and 2^-27 against c=1.
        case("c=1 + 16 x 2^-25", vec![(tiny, pow2(-13_i32)); 16], 1.0),
        case("c=1 + 16 x 2^-26", vec![(pow2(-13_i32), pow2(-13_i32)); 16], 1.0),
        case("c=1 + 16 x 2^-27", vec![(pow2(-13_i32), pow2(-14_i32)); 16], 1.0),
        case("c=1 + 8 x 2^-25", vec![(tiny, pow2(-13_i32)); 8], 1.0),
        case("c=1 + 4 x 2^-25", vec![(tiny, pow2(-13_i32)); 4], 1.0),
        // Whether k is summed in one fused step or in chunks rounded in between: two half-ulps either side of a
        // chunk boundary come to a whole ulp only if they meet before the sum is rounded.
        case("1@k0 + 2^-24@k1,k2", placed(&[(0, (1.0, 1.0)), (1, (tiny, tiny)), (2, (tiny, tiny))]), 0.0),
        case("1@k0 + 2^-24@k1,k4", placed(&[(0, (1.0, 1.0)), (1, (tiny, tiny)), (4, (tiny, tiny))]), 0.0),
        case("1@k0 + 2^-24@k3,k4", placed(&[(0, (1.0, 1.0)), (3, (tiny, tiny)), (4, (tiny, tiny))]), 0.0),
        case("1@k0 + 2^-24@k1,k8", placed(&[(0, (1.0, 1.0)), (1, (tiny, tiny)), (8, (tiny, tiny))]), 0.0),
        case("1@k0 + 2^-24@k7,k8", placed(&[(0, (1.0, 1.0)), (7, (tiny, tiny)), (8, (tiny, tiny))]), 0.0),
        case("1@k15 + 2^-24@k0,k8", placed(&[(15, (1.0, 1.0)), (0, (tiny, tiny)), (8, (tiny, tiny))]), 0.0),
        case("1@k8 + 2^-24@k0,k1", placed(&[(8, (1.0, 1.0)), (0, (tiny, tiny)), (1, (tiny, tiny))]), 0.0),
        case("c=1 + 2^-24@k0,k8", placed(&[(0, (tiny, tiny)), (8, (tiny, tiny))]), 1.0),
        case("c=1 + 2^-24@k0,k4", placed(&[(0, (tiny, tiny)), (4, (tiny, tiny))]), 1.0),
        case("c=1 + 2^-24@k0,k1", placed(&[(0, (tiny, tiny)), (1, (tiny, tiny))]), 1.0),
        // Cancellation in different chunks: whether a chunk's partial sum is normalised before the next is added.
        case(
            "2^24@k0 - 2^24@k8 + 1@k15",
            placed(&[(0, (4096.0, 4096.0)), (8, (-4096.0, 4096.0)), (15, (1.0, 1.0))]),
            0.0,
        ),
        case(
            "2^24@k0 - 2^24@k1 + 2^-10@k8",
            placed(&[(0, (4096.0, 4096.0)), (1, (-4096.0, 4096.0)), (8, (pow2(-5_i32), pow2(-5_i32)))]),
            0.0,
        ),
        // What becomes of bits below an operand format's precision: 0.75 of a tf32 ulp, and an f32 low bit.
        case("(1 + 2^-11 + 2^-12) x 1", vec![(1.0 + pow2(-11_i32) + pow2(-12_i32), 1.0)], 0.0),
        case("(1 + 2^-20) x 1", vec![(1.0 + pow2(-20_i32), 1.0)], 0.0),
        case("(1 + 2^-23) x (1 + 2^-23)", vec![(1.0 + pow2(-23_i32), 1.0 + pow2(-23_i32))], 0.0),
        // Overflow, infinity and not-a-number.
        case("65504 x 2 (f16 overflow)", vec![(65504.0, 2.0)], 0.0),
        case("inf x 0", vec![(f32::INFINITY, 0.0)], 0.0),
        case("inf - inf", vec![(f32::INFINITY, 1.0), (f32::NEG_INFINITY, 1.0)], 0.0),
    ]
}

/// The products `at` names, each at its own k, with zero products between them.
fn placed(at: &[(usize, (f32, f32))]) -> Vec<(f32, f32)> {
    let len = at.iter().map(|&(k, _)| k + 1).max().unwrap_or(0);
    let mut products = vec![(0.0_f32, 0.0_f32); len];
    for &(k, product) in at {
        if let Some(slot) = products.get_mut(k) {
            *slot = product;
        }
    }
    products
}

/// `value` rounded to f32.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "rounding the host's f64 answer to f32 gives the correctly rounded f32 answer, which is the point"
)]
const fn narrow(value: f64) -> f32 {
    value as f32
}

/// The exact sum of `case`, its products taken at the precision `kind` passes them in.
#[expect(clippy::float_arithmetic, reason = "the reference is the host's exact arithmetic")]
fn mma_exact(kind: Mma, case: &MmaCase) -> f64 {
    let products: f64 = case
        .products
        .iter()
        .map(|&(left, right)| kind.quantize(left) * kind.quantize(right))
        .sum();
    let accumulator = match kind {
        Mma::F16Acc => f16::from_f32(case.accumulator).to_f64(),
        Mma::F16 | Mma::Bf16 | Mma::Tf32 => f64::from(case.accumulator),
    };
    products + accumulator
}

/// The words `case` puts to the tensor cores: A's k elements and B's, packed into 8 words each, then the accumulator.
fn mma_input(kind: Mma, case: &MmaCase) -> Vec<u32> {
    let side = |pick: fn(&(f32, f32)) -> f32| -> Vec<u32> {
        let mut elements: Vec<u32> = case.products.iter().map(|product| kind.element(pick(product))).collect();
        elements.resize(kind.depth(), 0);
        match kind {
            Mma::Tf32 => elements,
            Mma::F16 | Mma::Bf16 | Mma::F16Acc => elements
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[low, high]| low | high.rotate_left(16))
                .collect(),
        }
    };
    [
        side(|product| product.0),
        side(|product| product.1),
        vec![kind.accumulator(case.accumulator)],
    ]
    .concat()
}

/// The PTX module holding `entry` for `sm_<arch>`, written to PTX ISA `version`.
fn module(version: &str, arch: u32, entry: &str) -> String {
    format!(".version {version}\n.target sm_{arch}\n.address_size 64\n\n{entry}")
}

/// The kernel running `op` once per thread, each thread's value in a 64-bit slot.
#[expect(clippy::single_call_fn, reason = "the one writer of an elementwise kernel")]
fn elementwise(op: &Probe) -> String {
    let (registers, load, store) = match op.kind {
        Kind::F16 => (
            ".reg .b16 %h<3>;",
            "ld.global.b16 %h1, [%rd4];",
            "st.global.b16 [%rd5], %h2;",
        ),
        Kind::F32 => (
            ".reg .f32 %f<3>;",
            "ld.global.f32 %f1, [%rd4];",
            "st.global.f32 [%rd5], %f2;",
        ),
        Kind::F64 => (
            ".reg .f64 %fd<3>;",
            "ld.global.f64 %fd1, [%rd4];",
            "st.global.f64 [%rd5], %fd2;",
        ),
    };
    let body = op.body;
    let entry = format!(
        ".visible .entry probe(.param .u64 pin, .param .u64 pout)
{{
  .reg .b32 %r<2>;
  .reg .b64 %rd<6>;
  {registers}
  ld.param.u64 %rd1, [pin];
  ld.param.u64 %rd2, [pout];
  cvta.to.global.u64 %rd1, %rd1;
  cvta.to.global.u64 %rd2, %rd2;
  mov.u32 %r1, %tid.x;
  mul.wide.u32 %rd3, %r1, 8;
  add.s64 %rd4, %rd1, %rd3;
  add.s64 %rd5, %rd2, %rd3;
  {load}
  {body}
  {store}
  ret;
}}
"
    );
    module(op.ptx_version, op.arch, &entry)
}

/// `bits`, a value whose sign bit is `sign`, placed so that neighbouring values are one apart.
fn ordered(bits: u64, sign: u64) -> i128 {
    let magnitude = i128::from(bits & !sign);
    if bits & sign == 0 {
        magnitude
    } else {
        0_i128.saturating_sub(magnitude)
    }
}

/// FNV-1a over `words`, to compare a probe's answers at a glance.
fn fnv(words: &[u64]) -> u64 {
    words.iter().fold(0xcbf2_9ce4_8422_2325, |hash, word| {
        (hash ^ word).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Load `ptx` and run its `probe` once, `threads` wide, over `input`, giving back `out_len` elements.
#[expect(
    unsafe_code,
    reason = "launching a kernel is unsafe in cuda-core; the block says why this launch is sound"
)]
fn run<In, Out>(
    ctx: &Arc<Context>,
    stream: &Arc<Stream>,
    ptx: &str,
    input: &[In],
    out_len: usize,
    threads: u32,
) -> Result<Vec<Out>, DriverError>
where
    In: DeviceCopy + Copy,
    Out: DeviceCopy + Copy + Default,
{
    let function = ctx.load_module_from_ptx_src(ptx)?.load_function("probe")?;
    let device_in = DeviceBuffer::from_host(stream, input)?;
    let device_out = DeviceBuffer::<Out>::zeroed(stream, out_len)?;
    let mut pin = device_in.cu_deviceptr();
    let mut pout = device_out.cu_deviceptr();
    let mut params = [(&raw mut pin).cast::<c_void>(), (&raw mut pout).cast::<c_void>()];
    // SAFETY: every `probe` takes two global pointers, to `input` and to `out_len` elements, which the buffers are,
    // and reads and writes no further than its `threads` slots.
    unsafe { launch_kernel_on_stream(&function, (1, 1, 1), (threads, 1, 1), 0, stream, &mut params) }?;
    stream.synchronize()?;
    device_out.to_host_vec(stream)
}

/// What `op` answers on the device, as each answer's bits beside its error in ulps.
fn measure(ctx: &Arc<Context>, stream: &Arc<Stream>, op: &Probe) -> Result<Vec<(u64, i128)>, DriverError> {
    let input: Vec<u64> = INPUTS
        .iter()
        .map(|bits| op.kind.encode(f64::from(f32::from_bits(*bits))))
        .collect();
    let answers: Vec<u64> = run(ctx, stream, &elementwise(op), &input, input.len(), 32)?;
    let sign = op.kind.sign();
    Ok(answers
        .iter()
        .zip(&input)
        .map(|(answer, operand)| {
            let want = op.kind.encode((op.reference)(op.kind.decode(*operand)));
            (*answer, ordered(*answer, sign).saturating_sub(ordered(want, sign)))
        })
        .collect())
}

/// Write each elementwise probe's answers: a hash of them, how many are exact, the worst error, and every error.
#[expect(clippy::single_call_fn, reason = "one table of the report")]
fn report_ops(out: &mut StdoutLock<'_>, ctx: &Arc<Context>, stream: &Arc<Stream>) -> Result<(), Box<dyn Error>> {
    for op in &OPS {
        let name = op.name;
        match measure(ctx, stream, op) {
            Ok(answers) => {
                let words: Vec<u64> = answers.iter().map(|answer| answer.0).collect();
                let ulps: Vec<String> = answers.iter().map(|answer| answer.1.to_string()).collect();
                let exact = answers.iter().filter(|answer| answer.1 == 0).count();
                let worst = answers
                    .iter()
                    .map(|answer| answer.1.saturating_abs())
                    .max()
                    .unwrap_or(0);
                let hash = fnv(&words);
                writeln!(out, "  {name:<24} fnv={hash:016x} exact={exact:>2}/32 max_ulps={worst}")?;
                writeln!(out, "    ulps [{}]", ulps.join(", "))?;
            }
            Err(error) => writeln!(out, "  {name:<24} refused: {error}")?,
        }
    }
    Ok(())
}

/// Write each tensor-core case's answer beside the sum rounded to nearest.
#[expect(clippy::single_call_fn, reason = "one table of the report")]
fn report_mma(out: &mut StdoutLock<'_>, ctx: &Arc<Context>, stream: &Arc<Stream>) -> Result<(), Box<dyn Error>> {
    for kind in Mma::ALL {
        let ptx = module("7.0", 80, &kind.entry());
        writeln!(out, "  mma {}:", kind.name())?;
        let mut words = Vec::new();
        for case in mma_cases() {
            let name = case.name;
            if case.products.len() > kind.depth() {
                continue;
            }
            match run::<u32, u32>(ctx, stream, &ptx, &mma_input(kind, &case), 1, 32) {
                Ok(answer) => {
                    let got = kind.answer(answer.first().copied().unwrap_or_default());
                    words.push(u64::from(got));
                    let nearest = kind.nearest(mma_exact(kind, &case));
                    let mark = if got == nearest { "rn" } else { "differs" };
                    writeln!(out, "    {name:<30} got={got:08x} round-to-nearest={nearest:08x} {mark}")?;
                }
                Err(error) => writeln!(out, "    {name:<30} refused: {error}")?,
            }
        }
        writeln!(out, "    fnv of every answer: {:016x}", fnv(&words))?;
    }
    Ok(())
}

/// Write the hash of each probe's answers when its module names each target from its own onwards.
#[expect(clippy::single_call_fn, reason = "one table of the report")]
fn report_targets(out: &mut StdoutLock<'_>, ctx: &Arc<Context>, stream: &Arc<Stream>) -> Result<(), Box<dyn Error>> {
    writeln!(out, "  across PTX targets (target:fnv of the answers):")?;
    for op in &OPS {
        let mut line = format!("    {:<24}", op.name);
        for &(arch, ptx_version) in TARGETS.iter().filter(|target| target.0 >= op.arch) {
            match measure(
                ctx,
                stream,
                &Probe {
                    arch,
                    ptx_version,
                    ..*op
                },
            ) {
                Ok(answers) => {
                    let words: Vec<u64> = answers.iter().map(|answer| answer.0).collect();
                    write!(line, " {arch}:{:016x}", fnv(&words))?;
                }
                Err(_) => write!(line, " {arch}:refused")?,
            }
        }
        writeln!(out, "{line}")?;
    }
    let mut line = format!("    {:<24}", "mma (every case)");
    for &(arch, ptx_version) in TARGETS.iter().filter(|target| target.0 >= 80) {
        let ptx = module(ptx_version, arch, &Mma::F16.entry());
        let answers: Result<Vec<u64>, DriverError> = mma_cases()
            .iter()
            .map(|case| run::<u32, u32>(ctx, stream, &ptx, &mma_input(Mma::F16, case), 1, 32))
            .map(|answer| answer.map(|words| u64::from(words.first().copied().unwrap_or_default())))
            .collect();
        match answers {
            Ok(words) => write!(line, " {arch}:{:016x}", fnv(&words))?,
            Err(_) => write!(line, " {arch}:refused")?,
        }
    }
    writeln!(out, "{line}")?;
    Ok(())
}

/// Write every table for the device `ordinal`.
#[expect(clippy::single_call_fn, reason = "the report of one device")]
fn report_device(out: &mut StdoutLock<'_>, ordinal: usize) -> Result<(), Box<dyn Error>> {
    let ctx = Context::new(ordinal)?;
    let stream = ctx.new_stream()?;
    let (major, minor) = ctx.compute_capability()?;
    writeln!(out, "device {ordinal}: {} sm_{major}{minor}", ctx.name()?)?;
    report_ops(out, &ctx, &stream)?;
    report_mma(out, &ctx, &stream)?;
    report_targets(out, &ctx, &stream)?;
    Ok(())
}

/// The CUDA version the installed driver supports, such as 13000 for 13.0.
#[expect(clippy::single_call_fn, reason = "the one question put to the driver itself")]
#[expect(
    unsafe_code,
    reason = "the driver API is unsafe to call; the block says why this call is sound"
)]
fn driver_version() -> Result<i32, DriverError> {
    let mut version = 0_i32;
    // SAFETY: cuDriverGetVersion writes one int through the pointer it is given, which points at `version`.
    unsafe { cuda_core::sys::cuDriverGetVersion(&raw mut version) }.result()?;
    Ok(version)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "cuda driver version: {}", driver_version()?)?;
    for ordinal in 0..usize::try_from(Context::device_count()?)? {
        report_device(&mut out, ordinal)?;
    }
    Ok(())
}
