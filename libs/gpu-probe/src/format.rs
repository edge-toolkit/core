//! The texture formats the probes with no shader of their own write and sample, and how each stores a value.

/// The largest code an 8-bit normalized channel holds.
const UNORM8_MAX: f64 = 255.0;

/// The largest code a 10-bit normalized channel holds.
const UNORM10_MAX: f64 = 1023.0;

/// The mantissa bits of an IEEE half, which a 16-bit float channel holds.
const HALF_MANTISSA: u32 = 10;

/// The mantissa bits of the unsigned 11-bit float a red or green channel of `rg11b10ufloat` holds.
const UFLOAT11_MANTISSA: u32 = 6;

/// The exponent field of a small float that marks infinity: every bit of its five set.
const SMALL_FLOAT_INFINITE_EXPONENT: u32 = 31;

/// The exponent a small float's smallest normal value has, which its subnormal values are scaled by too.
const SMALL_FLOAT_MIN_EXPONENT: i32 = -14;

/// The offset a small float's five-bit exponent field holds its exponent at.
const SMALL_FLOAT_BIAS: i32 = 15;

/// The offset an f64's exponent field holds its exponent at.
const DOUBLE_BIAS: i32 = 1023;

/// The linear value below which the sRGB curve is a straight line.
const SRGB_LINEAR_LIMIT: f64 = 0.003_130_8;

/// The slope of the straight part of the sRGB curve.
const SRGB_SLOPE: f64 = 12.92;

/// The encoded value below which the sRGB curve is a straight line.
const SRGB_ENCODED_LIMIT: f64 = 0.040_45;

/// The exponent of the curved part of the sRGB curve.
const SRGB_GAMMA: f64 = 2.4;

/// The offset the curved part of the sRGB curve is shifted by.
const SRGB_OFFSET: f64 = 0.055;

/// A texture format a probe writes or samples, named as WebGPU names it.
///
/// Every format's texel holds its red channel in its lowest bits, so the first four bytes of a texel read in little-
/// endian order give the red channel under a mask, which is how [`Format::red`] reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Format {
    /// Four 8-bit normalized channels.
    Rgba8Unorm,
    /// Four 8-bit normalized channels, the colour ones holding an sRGB encoding of the value.
    Rgba8UnormSrgb,
    /// Three 10-bit normalized colour channels and a 2-bit normalized alpha.
    Rgb10a2Unorm,
    /// Unsigned floats: 11 bits for red and green, 10 for blue, and no alpha.
    Rg11b10Ufloat,
    /// Four IEEE half-precision floats.
    Rgba16Float,
}

impl Format {
    /// The name WebGPU gives the format.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rgba8Unorm => "rgba8unorm",
            Self::Rgba8UnormSrgb => "rgba8unorm-srgb",
            Self::Rgb10a2Unorm => "rgb10a2unorm",
            Self::Rg11b10Ufloat => "rg11b10ufloat",
            Self::Rgba16Float => "rgba16float",
        }
    }

    /// How many bytes one texel of the format takes.
    #[must_use]
    pub const fn texel_bytes(self) -> u32 {
        match self {
            Self::Rgba16Float => 8,
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb | Self::Rgb10a2Unorm | Self::Rg11b10Ufloat => 4,
        }
    }

    /// How many bits of the texel the red channel takes, starting from the lowest.
    const fn red_bits(self) -> u32 {
        match self {
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb => 8,
            Self::Rgb10a2Unorm => 10,
            Self::Rg11b10Ufloat => 11,
            Self::Rgba16Float => 16,
        }
    }

    /// The code the red channel holds, read from the first four bytes of a texel as the GPU laid them out.
    #[must_use]
    #[expect(
        clippy::little_endian_bytes,
        reason = "WebGPU lays a texel's channels out in little-endian order on every platform"
    )]
    pub fn red(self, first_word: [u8; 4]) -> u32 {
        u32::from_le_bytes(first_word) & low_bits(self.red_bits())
    }

    /// The code the format holds `value` as, rounded correctly: to the nearest code, a tie to the even one.
    ///
    /// A normalized format holds the value clamped to [0, 1], and an sRGB one its sRGB encoding. A float format's value
    /// is taken to be zero or more; one past the format's largest is infinity.
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "the code a value is held as is a product of the value and the format's scale"
    )]
    pub fn encode(self, value: f64) -> u32 {
        match self {
            Self::Rgba8Unorm => whole(value.clamp(0.0, 1.0) * UNORM8_MAX),
            Self::Rgba8UnormSrgb => whole(srgb_encode(value.clamp(0.0, 1.0)) * UNORM8_MAX),
            Self::Rgb10a2Unorm => whole(value.clamp(0.0, 1.0) * UNORM10_MAX),
            Self::Rg11b10Ufloat => small_float(value, UFLOAT11_MANTISSA),
            Self::Rgba16Float => small_float(value, HALF_MANTISSA),
        }
    }

    /// The value the red channel's `code` stands for, as a sampler or a blend reads it.
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "a normalized code stands for its quotient by the format's scale"
    )]
    pub fn decode(self, code: u32) -> f64 {
        match self {
            Self::Rgba8Unorm => f64::from(code) / UNORM8_MAX,
            Self::Rgba8UnormSrgb => srgb_decode(f64::from(code) / UNORM8_MAX),
            Self::Rgb10a2Unorm => f64::from(code) / UNORM10_MAX,
            Self::Rg11b10Ufloat => small_float_value(code, UFLOAT11_MANTISSA),
            Self::Rgba16Float => small_float_value(code, HALF_MANTISSA),
        }
    }

    /// The bytes of one texel whose red channel holds `code`, its green and blue zero, and its alpha, if any, one.
    #[must_use]
    #[expect(
        clippy::little_endian_bytes,
        reason = "WebGPU lays a texel's channels out in little-endian order on every platform"
    )]
    pub fn texel(self, code: u32) -> Vec<u8> {
        let red = code & low_bits(self.red_bits());
        let word = match self {
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb => red | 0xff00_0000,
            Self::Rgb10a2Unorm => red | 0xc000_0000,
            Self::Rg11b10Ufloat => red,
            Self::Rgba16Float => {
                let one_alpha: u64 = 0x3c00_0000_0000_0000;
                return (u64::from(red) | one_alpha).to_le_bytes().to_vec();
            }
        };
        word.to_le_bytes().to_vec()
    }
}

/// A word with its lowest `count` bits set, `count` being 16 or fewer.
fn low_bits(count: u32) -> u32 {
    1_u32.checked_shl(count).map_or(u32::MAX, |bit| bit.saturating_sub(1))
}

/// `value`, a whole number from zero to `u32::MAX` once rounded, rounded to the nearest one, a tie to the even one.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is rounded to a whole number in range first, and std has no other conversion from f64"
)]
const fn whole(value: f64) -> u32 {
    value.round_ties_even() as u32
}

/// The sRGB encoding of `linear`, a value from zero to one.
#[expect(
    clippy::float_arithmetic,
    clippy::single_call_fn,
    reason = "the sRGB curve is float arithmetic, kept beside its inverse"
)]
fn srgb_encode(linear: f64) -> f64 {
    if linear <= SRGB_LINEAR_LIMIT {
        linear * SRGB_SLOPE
    } else {
        (1.0 + SRGB_OFFSET).mul_add(linear.powf(SRGB_GAMMA.recip()), -SRGB_OFFSET)
    }
}

/// The linear value whose sRGB encoding is `encoded`, a value from zero to one.
#[expect(
    clippy::float_arithmetic,
    clippy::single_call_fn,
    reason = "the sRGB curve is float arithmetic, kept beside its inverse"
)]
fn srgb_decode(encoded: f64) -> f64 {
    if encoded <= SRGB_ENCODED_LIMIT {
        encoded / SRGB_SLOPE
    } else {
        ((encoded + SRGB_OFFSET) / (1.0 + SRGB_OFFSET)).powf(SRGB_GAMMA)
    }
}

/// The bits of `value`, zero or more, as a float with a five-bit exponent, `mantissa` mantissa bits and no sign.
///
/// The value is scaled by a power of two to put its last kept bit at one, which is exact, and rounded once. The rounded
/// mantissa carries its leading one into the exponent field, so a value that rounds up to the next power of two, and a
/// subnormal one that rounds up to the smallest normal, each land on the right bits with no case of its own. Past the
/// largest finite value the bits are infinity's.
#[expect(
    clippy::float_arithmetic,
    reason = "scaling by a power of two is exact, and it puts the rounding where the format rounds"
)]
fn small_float(value: f64, mantissa: u32) -> u32 {
    let field = (value.to_bits() >> 52_u32) & 0x7ff;
    let exponent = i32::try_from(field)
        .unwrap_or(i32::MAX)
        .saturating_sub(DOUBLE_BIAS)
        .max(SMALL_FLOAT_MIN_EXPONENT);
    let shift = i32::try_from(mantissa).unwrap_or(0_i32).saturating_sub(exponent);
    let quantum = whole(value * 2.0_f64.powi(shift));
    let biased = u32::try_from(exponent.saturating_add(SMALL_FLOAT_BIAS).saturating_sub(1)).unwrap_or(0_u32);
    let bits = (biased << mantissa).saturating_add(quantum);
    bits.min(SMALL_FLOAT_INFINITE_EXPONENT << mantissa)
}

/// The value the bits of a float with a five-bit exponent, `mantissa` mantissa bits and no sign stand for.
#[expect(
    clippy::float_arithmetic,
    reason = "a float's value is its mantissa scaled by a power of two"
)]
fn small_float_value(bits: u32, mantissa: u32) -> f64 {
    let fraction = bits & low_bits(mantissa);
    let field = bits >> mantissa;
    let width = i32::try_from(mantissa).unwrap_or(0_i32);
    if field == 0 {
        return f64::from(fraction) * 2.0_f64.powi(SMALL_FLOAT_MIN_EXPONENT.saturating_sub(width));
    }
    if field >= SMALL_FLOAT_INFINITE_EXPONENT {
        return f64::INFINITY;
    }
    let exponent = i32::try_from(field).unwrap_or(0_i32).saturating_sub(SMALL_FLOAT_BIAS);
    let significand = f64::from(fraction.saturating_add(1_u32 << mantissa));
    significand * 2.0_f64.powi(exponent.saturating_sub(width))
}
