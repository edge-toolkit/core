//! Probes of the matrix units: one cooperative-matrix multiply-add each, through wgpu's `wgpu_cooperative_matrix`.
//!
//! A cooperative-matrix multiply-add, `D = A * B + C`, runs on the hardware a vendor builds for matrices -- NVIDIA's
//! tensor cores, Apple's simdgroup matrices -- and nothing in it is pinned down past the sum being near the exact one.
//! How many bits below the accumulator's last place survive the products being lined up, how many of the k products
//! are summed in one step before a rounding, whether each step's sum is normalised before the next is added, and
//! whether a product is rounded before it is added are the hardware's own choices, and they differ between generations
//! of one vendor's GPUs where every shader probe answers alike. Each probe puts its k values in both the first row and
//! the first column of A, the other elements zero, and likewise B, then reads back D's first element, the sum of the k
//! products. A backend that reads the tiles transposed -- Metal's does, against Vulkan's -- reads the same sum.
//!
//! The probes come in two shapes, because no device offers every shape: 16x16x16 with `f16` operands, which NVIDIA's
//! Vulkan driver offers and Metal does not, and 8x8x8 in `f32` throughout, which Metal offers. A device refuses each
//! probe of a shape it does not offer.
#![expect(
    clippy::single_call_fn,
    reason = "each step of writing a probe's shader and reading its answers is a named function"
)]

use half::f16;

/// The interface a signature was measured through, one cooperative-matrix multiply-add per probe.
///
/// Each probe ran in a WGSL compute shader under wgpu's `wgpu_cooperative_matrix` extension, which only wgpu offers,
/// natively, through Vulkan's `VK_KHR_cooperative_matrix` and Metal's simdgroup matrices.
pub const WGPU_COOPERATIVE_MATRIX: &str = "wgpu-cooperative-matrix";

/// How many probes the set makes.
pub const MATRIX_COUNT: usize = 16;

/// `2^-12`, half an ulp of 1 in `f32` when squared.
const TINY: f32 = f32::from_bits(0x3980_0000);

/// `2^-13`, which times [`TINY`] is a quarter of an ulp of 1 in `f32`.
const TINIER: f32 = f32::from_bits(0x3900_0000);

/// `1 + 2^-12`, whose square `1 + 2^-11 + 2^-24` falls exactly halfway between two `f32` values.
const ULP_BASE: f32 = f32::from_bits(0x3f80_0800);

/// `-(1 + 2^-11)`, the negation of [`ULP_BASE`] squared and rounded to `f32`.
const NEGATED_SQUARE: f32 = f32::from_bits(0xbf80_1000);

/// `k` products of [`TINY`] and [`TINIER`], each a quarter of an ulp of 1, at k = 0 onwards.
macro_rules! quarter_ulps {
    ($($k:literal)*) => {
        &[$(($k, TINY, TINIER)),*]
    };
}

/// Every matrix probe, in output order.
///
/// The first three of the 16x16x16 probes add quarter-ulps to C = 1: a device that keeps one bit below the last place
/// when lining the products up loses them all, and one that keeps two keeps them. The next three put two half-ulps
/// either side of k = 8, or of none, beside a 1: a device that sums the k products in chunks of 8, truncating each
/// chunk's sum, loses both, and one that sums them all in one step keeps their whole ulp. The seventh cancels `2^24`
/// within the first chunk and adds `2^-10` in the second: a device that normalises each chunk's sum keeps the `2^-10`,
/// and one that lines every product up against the largest loses it. The next two do the like into an `f16`
/// accumulator, where the first chunk's `2^24` alone overflows it.
///
/// The 8x8x8 probes ask the same of `f32` operands, with chunks of 4 in place of 8, and two things more: whether a
/// product is rounded to `f32` before it is added, which cancelling the square of `1 + 2^-12` against its rounded value
/// shows, and how three half-ulps summed before a 1 are rounded once it arrives.
pub const MATRIX_PROBES: [MatrixProbe; MATRIX_COUNT] = [
    half(
        "c1_16_quarter_ulps",
        Accumulator::F32,
        quarter_ulps!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15),
        1.0,
    ),
    half(
        "c1_8_quarter_ulps",
        Accumulator::F32,
        quarter_ulps!(0 1 2 3 4 5 6 7),
        1.0,
    ),
    half("c1_4_quarter_ulps", Accumulator::F32, quarter_ulps!(0 1 2 3), 1.0),
    half(
        "one_k0_half_ulps_k1_k8",
        Accumulator::F32,
        &[(0, 1.0, 1.0), (1, TINY, TINY), (8, TINY, TINY)],
        0.0,
    ),
    half(
        "one_k0_half_ulps_k7_k8",
        Accumulator::F32,
        &[(0, 1.0, 1.0), (7, TINY, TINY), (8, TINY, TINY)],
        0.0,
    ),
    half(
        "c1_half_ulps_k0_k8",
        Accumulator::F32,
        &[(0, TINY, TINY), (8, TINY, TINY)],
        1.0,
    ),
    half(
        "cancel_k0_k1_small_k8",
        Accumulator::F32,
        &[(0, 4096.0, 4096.0), (1, -4096.0, 4096.0), (8, 0.031_25, 0.031_25)],
        0.0,
    ),
    half(
        "f16_cancel_k0_k8_one_k15",
        Accumulator::F16,
        &[(0, 4096.0, 4096.0), (8, -4096.0, 4096.0), (15, 1.0, 1.0)],
        0.0,
    ),
    half(
        "f16_cancel_k0_k1_small_k8",
        Accumulator::F16,
        &[(0, 4096.0, 4096.0), (1, -4096.0, 4096.0), (8, 0.031_25, 0.031_25)],
        0.0,
    ),
    single("f32x8_c1_8_quarter_ulps", quarter_ulps!(0 1 2 3 4 5 6 7), 1.0),
    single("f32x8_c1_4_quarter_ulps", quarter_ulps!(0 1 2 3), 1.0),
    single(
        "f32x8_one_k0_half_ulps_k3_k4",
        &[(0, 1.0, 1.0), (3, TINY, TINY), (4, TINY, TINY)],
        0.0,
    ),
    single("f32x8_c1_half_ulps_k0_k4", &[(0, TINY, TINY), (4, TINY, TINY)], 1.0),
    single(
        "f32x8_cancel_k0_k1_small_k4",
        &[(0, 4096.0, 4096.0), (1, -4096.0, 4096.0), (4, 0.031_25, 0.031_25)],
        0.0,
    ),
    single("f32x8_unrounded_product", &[(0, ULP_BASE, ULP_BASE)], NEGATED_SQUARE),
    single(
        "f32x8_half_ulps_then_one",
        &[(0, TINY, TINY), (1, TINY, TINY), (2, TINY, TINY), (7, 1.0, 1.0)],
        0.0,
    ),
];

/// Each probe with more than one known answer, by name, with each answer beside its meaning.
pub(crate) const CHOICES: [(&str, &[(u32, &str)]); 9] = [
    (
        "c1_16_quarter_ulps",
        &[(0x3f80_0000, "one_extra_bit"), (0x3f80_0004, "two_extra_bits")],
    ),
    (
        "c1_8_quarter_ulps",
        &[(0x3f80_0000, "one_extra_bit"), (0x3f80_0002, "two_extra_bits")],
    ),
    (
        "c1_4_quarter_ulps",
        &[(0x3f80_0000, "one_extra_bit"), (0x3f80_0001, "two_extra_bits")],
    ),
    (
        "one_k0_half_ulps_k1_k8",
        &[(0x3f80_0000, "chunks_of_8"), (0x3f80_0001, "fused")],
    ),
    (
        "one_k0_half_ulps_k7_k8",
        &[(0x3f80_0000, "chunks_of_8"), (0x3f80_0001, "fused")],
    ),
    (
        "c1_half_ulps_k0_k8",
        &[(0x3f80_0000, "chunks_of_8"), (0x3f80_0001, "fused")],
    ),
    (
        "cancel_k0_k1_small_k8",
        &[(0x3a80_0000, "normalised_per_chunk"), (0, "aligned_to_largest")],
    ),
    (
        "f16_cancel_k0_k8_one_k15",
        &[(0x7c00, "chunk_overflowed"), (0x3c00, "fused")],
    ),
    (
        "f16_cancel_k0_k1_small_k8",
        &[(0x1400, "normalised_per_chunk"), (0, "aligned_to_largest")],
    ),
];

/// The type of a matrix probe's accumulator, C and D.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Accumulator {
    /// `f32`.
    F32,
    /// `f16`.
    F16,
}

/// The shape of a matrix probe's tiles, and the type of A and B.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Shape {
    /// 16x16x16, with `f16` A and B.
    F16x16,
    /// 8x8x8, with `f32` A and B; the accumulator is `f32` too.
    F32x8,
}

impl Shape {
    /// How many rows and columns each tile has, and so how many products each element of D sums.
    const fn size(self) -> usize {
        match self {
            Self::F16x16 => 16,
            Self::F32x8 => 8,
        }
    }

    /// How many elements of each tile each of the subgroup's 32 lanes fills.
    const fn share(self) -> usize {
        match self {
            Self::F16x16 => 8,
            Self::F32x8 => 2,
        }
    }
}

/// One matrix probe: the products at each k, A's value times B's, with the products not named zero, and C.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct MatrixProbe {
    /// The probe's name.
    pub name: &'static str,
    /// The tiles' shape and A and B's type.
    pub shape: Shape,
    /// The accumulator's type.
    pub accumulator: Accumulator,
    /// Each product's k, A's value at it and B's.
    pub products: &'static [(usize, f32, f32)],
    /// The value of every element of C.
    pub addend: f32,
}

/// A 16x16x16 probe of `f16` operands into `accumulator`.
const fn half(
    name: &'static str,
    accumulator: Accumulator,
    products: &'static [(usize, f32, f32)],
    addend: f32,
) -> MatrixProbe {
    MatrixProbe {
        name,
        shape: Shape::F16x16,
        accumulator,
        products,
        addend,
    }
}

/// An 8x8x8 probe in `f32` throughout.
const fn single(name: &'static str, products: &'static [(usize, f32, f32)], addend: f32) -> MatrixProbe {
    MatrixProbe {
        name,
        shape: Shape::F32x8,
        accumulator: Accumulator::F32,
        products,
        addend,
    }
}

/// Each matrix probe's name.
pub(crate) fn names() -> Vec<&'static str> {
    MATRIX_PROBES.iter().map(|probe| probe.name).collect()
}

/// Each matrix probe's A and B values at every k of its shape, zero where the probe names no product.
fn operands(probe: &MatrixProbe) -> (Vec<f32>, Vec<f32>) {
    (0..probe.shape.size())
        .map(|depth| {
            probe
                .products
                .iter()
                .find(|&&(position, _, _)| position == depth)
                .map_or((0.0, 0.0), |&(_, in_a, in_b)| (in_a, in_b))
        })
        .unzip()
}

/// `value` as a WGSL literal of the type whose suffix is `suffix`, written in full so the literal is exact.
fn literal(value: f32, suffix: &str) -> String {
    format!("{}{suffix}", f64::from(value))
}

/// Each matrix probe's name beside the WGSL compute shader that runs it, in output order.
///
/// Every shader reads [`INPUTS`](crate::INPUTS) from an 8-element `f32` storage buffer at binding 0, scaling C by its 1
/// so that nothing is folded at compile time, and writes D's first element to a 1-element `u32` storage buffer at
/// binding 1: an `f32`'s bits, or an `f16`'s in the low half. One subgroup of 32 runs it, filling the tiles in
/// workgroup memory, each lane an equal share of each.
#[must_use]
pub fn matrix_shaders() -> Vec<(&'static str, String)> {
    MATRIX_PROBES.iter().map(|probe| (probe.name, shader(probe))).collect()
}

/// What a matrix probe's shader is written from: its types, its tile, and the values it fills the tiles with.
struct Kernel {
    /// The `enable` directives the operand type needs beyond the cooperative-matrix one.
    enable: &'static str,
    /// A and B's WGSL type.
    operand: &'static str,
    /// C and D's WGSL type.
    accumulator: &'static str,
    /// The expression of D's first element as the `u32` written back.
    bits: &'static str,
    /// The tiles' rows and columns.
    size: usize,
    /// How many elements of each tile each lane fills.
    share: usize,
    /// A's k values, as WGSL literals.
    a_values: String,
    /// B's k values, as WGSL literals.
    b_values: String,
    /// C's value, as a WGSL literal.
    addend: String,
}

/// The WGSL compute shader that runs `probe`.
fn shader(probe: &MatrixProbe) -> String {
    let (left, right) = operands(probe);
    let (operand, operand_suffix, enable) = match probe.shape {
        Shape::F16x16 => ("f16", "h", "enable f16;\n"),
        Shape::F32x8 => ("f32", "f", ""),
    };
    let listed = |values: &[f32]| -> String {
        values
            .iter()
            .map(|value| literal(*value, operand_suffix))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (accumulator, suffix, bits) = match probe.accumulator {
        Accumulator::F32 => ("f32", "f", "bitcast<u32>(cTile[0])"),
        Accumulator::F16 => ("f16", "h", "pack2x16float(vec2<f32>(f32(cTile[0]), 0.0))"),
    };
    wgsl(&Kernel {
        enable,
        operand,
        accumulator,
        bits,
        size: probe.shape.size(),
        share: probe.shape.share(),
        a_values: listed(&left),
        b_values: listed(&right),
        addend: literal(probe.addend, suffix),
    })
}

/// The WGSL of `kernel`.
fn wgsl(kernel: &Kernel) -> String {
    let Kernel {
        enable,
        operand,
        accumulator,
        bits,
        size,
        share,
        a_values,
        b_values,
        addend,
    } = kernel;
    let elements = size.saturating_mul(*size);
    let tile = format!("coop_mat{size}x{size}");
    format!(
        "{enable}enable wgpu_cooperative_matrix;

@group(0) @binding(0) var<storage, read>       probeIn  : array<f32, 8>;
@group(0) @binding(1) var<storage, read_write> probeOut : array<u32, 1>;

var<workgroup> aTile : array<{operand}, {elements}>;
var<workgroup> bTile : array<{operand}, {elements}>;
var<workgroup> cTile : array<{accumulator}, {elements}>;

@compute @workgroup_size(32)
fn main(@builtin(local_invocation_index) lane : u32) {{
    var aAtK = array<{operand}, {size}>({a_values});
    var bAtK = array<{operand}, {size}>({b_values});
    let addend = {accumulator}(probeIn[3]) * {addend};
    for (var slot = lane * {share}u; slot < lane * {share}u + {share}u; slot = slot + 1u) {{
        let row = slot % {size}u;
        let column = slot / {size}u;
        var a = {operand}(0);
        var b = {operand}(0);
        if row == 0u {{
            a = aAtK[column];
            b = bAtK[column];
        }} else if column == 0u {{
            a = aAtK[row];
            b = bAtK[row];
        }}
        aTile[slot] = a;
        bTile[slot] = b;
        cTile[slot] = addend;
    }}
    workgroupBarrier();
    let a = coopLoad<{tile}<{operand}, A>>(&aTile[0]);
    let b = coopLoad<{tile}<{operand}, B>>(&bTile[0]);
    let c = coopLoad<{tile}<{accumulator}, C>>(&cTile[0]);
    let d = coopMultiplyAdd(a, b, c);
    workgroupBarrier();
    coopStore(d, &cTile[0]);
    workgroupBarrier();
    if lane == 0u {{
        probeOut[0] = {bits};
    }}
}}
"
    )
}

/// The bits of each matrix probe's sum, taken exactly and rounded once to nearest in its accumulator's type.
#[must_use]
#[expect(clippy::float_arithmetic, reason = "the reference is the host's exact sum")]
pub fn matrix_reference() -> Vec<u32> {
    MATRIX_PROBES
        .iter()
        .map(|probe| {
            let operand = |value: f32| match probe.shape {
                Shape::F16x16 => f16::from_f32(value).to_f64(),
                Shape::F32x8 => f64::from(value),
            };
            let products: f64 = probe
                .products
                .iter()
                .map(|&(_, in_a, in_b)| operand(in_a) * operand(in_b))
                .sum();
            match probe.accumulator {
                Accumulator::F32 => crate::nearest(products + f64::from(probe.addend)).to_bits(),
                Accumulator::F16 => {
                    let addend = f16::from_f32(probe.addend).to_f64();
                    u32::from(f16::from_f64(products + addend).to_bits())
                }
            }
        })
        .collect()
}
