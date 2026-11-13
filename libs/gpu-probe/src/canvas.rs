//! The canvas fingerprints, taken beside the probe sets because a browser's defences treat a canvas read apart.
//!
//! Two are taken, since a defence can cover one way of making and reading a canvas and not another. [`CANVAS`] is
//! a fixed scene on a canvas the page made, read with `getImageData` and hashed with [`pixel_hash`]; it records the
//! hash's four words as `lane0` to `lane3`. [`CANVAS_BROWSERLEAKS`] is the canvas fingerprint browserleaks.com takes,
//! drawn on a canvas a sandboxed frame made and read with `toDataURL`; it records the MD5 of that PNG as `md5`, the
//! value that site shows as its signature in the same browser.

/// The interface a page's own canvas scene, read with `getImageData`, is filed under.
pub const CANVAS: &str = "canvas";

/// The interface the canvas fingerprint browserleaks.com takes is filed under, read with `toDataURL` in a frame.
pub const CANVAS_BROWSERLEAKS: &str = "canvas-browserleaks";

/// The offset each of the hash's four lanes starts from.
const LANE_SEEDS: [u32; 4] = [0x811c_9dc5, 0x1b87_3593, 0x9e37_79b1, 0x85eb_ca77];

/// The odd multiplier each lane takes a byte in with.
const LANE_PRIMES: [u32; 4] = [16_777_619, 2_246_822_519, 3_266_489_917, 668_265_263];

/// How many bytes the lanes take between folds.
const FOLD_EVERY: usize = 64;

/// The width and height of the known scene, in pixels.
pub const KNOWN_SIZE: u8 = 64;

/// The width and height of each of the known scene's blocks, in pixels.
pub const KNOWN_BLOCK: u8 = 8;

/// How many blocks the known scene has across and down, which [`KNOWN_BLOCK`] pixels each make [`KNOWN_SIZE`].
const KNOWN_BLOCKS: u8 = 8;

/// The opaque colour the known scene's block `across` from the left and `down` from the top is filled with.
///
/// The known scene is blocks of flat opaque colour aligned to the pixel grid, which every renderer draws exactly, so a
/// read of it that gives back any other value was changed on the way out: noise is seen within a single visit, with no
/// unprotected browser to compare against.
#[must_use]
pub const fn known_colour(across: u8, down: u8) -> [u8; 3] {
    [
        across
            .wrapping_mul(37)
            .wrapping_add(down.wrapping_mul(11))
            .wrapping_add(20),
        across
            .wrapping_mul(59)
            .wrapping_add(down.wrapping_mul(23))
            .wrapping_add(70),
        across
            .wrapping_mul(17)
            .wrapping_add(down.wrapping_mul(101))
            .wrapping_add(130),
    ]
}

/// The RGBA values a faithful read of the whole known scene gives back, row by row.
#[must_use]
pub fn known_scene() -> Vec<u8> {
    let block = usize::from(KNOWN_BLOCK);
    (0..KNOWN_BLOCKS)
        .flat_map(|down| {
            let row: Vec<u8> = (0..KNOWN_BLOCKS)
                .flat_map(|across| {
                    let [red, green, blue] = known_colour(across, down);
                    [red, green, blue, u8::MAX].repeat(block)
                })
                .collect();
            row.repeat(block)
        })
        .collect()
}

/// How a read of the known scene differed from what was drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Alteration {
    /// How many of the values read back differ from the ones drawn.
    pub changed: usize,
    /// How many values were read back.
    pub total: usize,
    /// The [`pixel_hash`] of where and how the values differ, or nothing where none do.
    ///
    /// It tells one noise from another.
    pub pattern: String,
}

/// A read whose length is not that of the scene it read.
#[derive(Debug)]
#[non_exhaustive]
pub struct ReadLength {
    /// How many values were read back.
    pub read: usize,
    /// How many the scene holds.
    pub expected: usize,
}

impl core::fmt::Display for ReadLength {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "read {} values, not {}", self.read, self.expected)
    }
}

impl core::error::Error for ReadLength {}

/// How `read` differs from `expected`, value by value.
///
/// # Errors
///
/// [`ReadLength`] when `read` holds a different number of values from `expected`.
pub fn alteration(read: &[u8], expected: &[u8]) -> Result<Alteration, ReadLength> {
    if read.len() != expected.len() {
        return Err(ReadLength {
            read: read.len(),
            expected: expected.len(),
        });
    }
    let difference: Vec<u8> = read.iter().zip(expected).map(|(got, drawn)| got ^ drawn).collect();
    let changed = difference.iter().filter(|value| **value != 0).count();
    let pattern = if changed == 0 {
        String::default()
    } else {
        pixel_hash(&difference).map(|lane| format!("{lane:08x}")).join("-")
    };
    Ok(Alteration {
        changed,
        total: read.len(),
        pattern,
    })
}

/// A 128-bit hash of a canvas's pixels, as four 32-bit words.
///
/// Four independent multiply-xor lanes each take every byte, and after each 64 bytes the first two are folded into the
/// last two. It is the hash the registry's [`CANVAS`] entries were taken with, so a signature taken here is compared
/// with them as it is.
#[must_use]
pub fn pixel_hash(bytes: &[u8]) -> [u32; 4] {
    let mut lanes = LANE_SEEDS;
    for block in bytes.chunks(FOLD_EVERY) {
        for byte in block {
            for (lane, prime) in lanes.iter_mut().zip(LANE_PRIMES) {
                *lane = (*lane ^ u32::from(*byte)).wrapping_mul(prime);
            }
        }
        if block.len() == FOLD_EVERY {
            let [first, second, third, fourth] = lanes;
            lanes = [first, second, third ^ (first >> 13_u32), fourth ^ (second >> 7_u32)];
        }
    }
    lanes
}
