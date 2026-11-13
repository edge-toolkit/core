//! Probes that run no shader: a render pass clears a one-texel target to a value, and the texel is read back.
//!
//! The clear value is given as a float, and the driver or the GPU's output stage converts it to the target's format.
//! That conversion is left loose: rounding to the nearest code is preferred and not required, how closely the sRGB
//! curve is followed is not pinned down, and a half-precision subnormal or overflow may be kept or not. No shader
//! compiler stands between the value and the texel, so what this set tells apart is the hardware and the driver alone.

use crate::Format;

/// How many clears the set makes.
pub const CLEAR_COUNT: usize = 10;

/// Every clear, in output order.
///
/// The first three put the same 8-bit code a quarter, a half and three quarters of the way to the next, and the 10-bit
/// one three quarters: rounding gives the next code where truncating keeps the lower. The half-way one is only half-way
/// as near as an f64 holds it, so which way it goes says how the conversion was computed. The two sRGB ones take the
/// curve's curved and straight parts. The float ones put a value three quarters of the way between two codes, below the
/// smallest normal half, and above the largest finite one.
pub const CLEARS: [Clear; CLEAR_COUNT] = [
    Clear {
        name: "unorm8_quarter",
        format: Format::Rgba8Unorm,
        value: 10.25 / 255.0,
    },
    Clear {
        name: "unorm8_half",
        format: Format::Rgba8Unorm,
        value: 10.5 / 255.0,
    },
    Clear {
        name: "unorm8_three_quarters",
        format: Format::Rgba8Unorm,
        value: 10.75 / 255.0,
    },
    Clear {
        name: "unorm10_three_quarters",
        format: Format::Rgb10a2Unorm,
        value: 100.75 / 1023.0,
    },
    Clear {
        name: "srgb8_mid",
        format: Format::Rgba8UnormSrgb,
        value: 0.5,
    },
    Clear {
        name: "srgb8_dark",
        format: Format::Rgba8UnormSrgb,
        value: 0.002,
    },
    Clear {
        name: "ufloat11_rounding",
        format: Format::Rg11b10Ufloat,
        value: 1.0 + 0.75 / 64.0,
    },
    Clear {
        name: "half_rounding",
        format: Format::Rgba16Float,
        value: 1.0 + 0.75 / 1024.0,
    },
    Clear {
        name: "half_subnormal",
        format: Format::Rgba16Float,
        value: 1.0 / 1_048_576.0,
    },
    Clear {
        name: "half_overflow",
        format: Format::Rgba16Float,
        value: 70_000.0,
    },
];

/// For each clear with more than one answer worth naming: its name, then each answer's code beside what it means.
pub(crate) const CHOICES: [(&str, &[(u32, &str)]); 6] = [
    ("unorm8_three_quarters", &[(11, "rounded"), (10, "truncated")]),
    ("unorm10_three_quarters", &[(101, "rounded"), (100, "truncated")]),
    ("ufloat11_rounding", &[(0x3c1, "rounded"), (0x3c0, "truncated")]),
    ("half_rounding", &[(0x3c01, "rounded"), (0x3c00, "truncated")]),
    ("half_subnormal", &[(0x0010, "kept"), (0, "flushed")]),
    ("half_overflow", &[(0x7c00, "infinity"), (0x7bff, "clamped")]),
];

/// One clear: the target's format, and the value its red channel is cleared to.
///
/// The target's green and blue channels are cleared to zero and its alpha to one, and only red is read back.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Clear {
    /// The probe's name.
    pub name: &'static str,
    /// The format of the target cleared.
    pub format: Format,
    /// The value the target's red channel is cleared to.
    pub value: f64,
}

/// The code each clear's red channel holds when the value is converted correctly, in output order.
#[must_use]
pub fn clear_reference() -> Vec<u32> {
    CLEARS.iter().map(|clear| clear.format.encode(clear.value)).collect()
}
