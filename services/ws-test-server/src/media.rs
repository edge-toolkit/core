//! A camera and a microphone simulated by files, which a test points a media module at in place of the devices, and the
//! check of the camera frame and microphone clip the module stores from them.
//!
//! The camera is one PPM image, which reads as a single frame and then the end of the stream. The microphone is a WAV
//! recording a clip and a half long, so the module stores a whole clip and then the half one that ends it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use crate::device::{self, DeviceError};

/// The key a media module stores the latest camera frame under, as a JPEG, in its agent's bucket.
pub const FRAME_KEY: &str = "camera-frame.jpg";

/// The key a media module stores the latest microphone clip under, as a WAV file, in its agent's bucket.
pub const CLIP_KEY: &str = "microphone-clip.wav";

/// The simulated camera's frame size, in pixels.
const WIDTH: u16 = 64;
const HEIGHT: u16 = 48;

/// The simulated microphone's sample rate, which is also how many samples a module puts in one clip.
const RATE: u16 = 16_000;

/// How many samples the simulated recording holds: a clip and a half.
const SAMPLES: u16 = 24_000;

/// A simulated camera and microphone, as files.
#[non_exhaustive]
pub struct SimulatedMedia {
    /// The camera.
    camera: PathBuf,
    /// The microphone.
    microphone: PathBuf,
    /// The directory holding both, removed when this is dropped.
    _dir: TempDir,
}

impl SimulatedMedia {
    /// The camera: a PPM image.
    #[must_use]
    pub fn camera(&self) -> &Path {
        &self.camera
    }

    /// The microphone: a WAV recording.
    #[must_use]
    pub fn microphone(&self) -> &Path {
        &self.microphone
    }

    /// Write the camera's image and the microphone's recording.
    #[must_use]
    pub fn write() -> Self {
        let dir = TempDir::new().unwrap();
        let camera = dir.path().join("camera.ppm");
        let mut image = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
        for row in 0..HEIGHT {
            for column in 0..WIDTH {
                image.extend([u8::try_from(column * 4).unwrap(), u8::try_from(row * 5).unwrap(), 128]);
            }
        }
        fs_err::write(&camera, image).unwrap();
        let microphone = dir.path().join("microphone.wav");
        fs_err::write(&microphone, wav(&recording())).unwrap();
        Self {
            camera,
            microphone,
            _dir: dir,
        }
    }

    /// Check the module stored the camera's frame and the recording's last half clip.
    ///
    /// The frame must be a JPEG of the camera's size, and both must be under `storage_dir` within `budget`.
    ///
    /// # Errors
    ///
    /// When it did not, saying what it stored instead, so a test can stop the runner it started before it fails.
    pub fn verify(&self, storage_dir: &Path, budget: Duration) -> Result<(), DeviceError> {
        let frame = device::wait_for_stored(storage_dir, FRAME_KEY, budget, |bytes| bytes.starts_with(&[0xFF, 0xD8]))?;
        let size = jpeg_size(&frame)?;
        if size != (WIDTH, HEIGHT) {
            return Err(DeviceError::Malformed {
                key: FRAME_KEY,
                problem: format!("is {size:?}, not the camera's {:?}", (WIDTH, HEIGHT)),
            });
        }
        let samples = recording();
        let last_clip = wav(samples.get(usize::from(RATE)..).unwrap_or_default());
        device::wait_for_stored(storage_dir, CLIP_KEY, budget, |bytes| bytes == last_clip).map(drop)
    }
}

/// The simulated recording's samples, each different from its neighbours so a clip from the wrong place is caught.
fn recording() -> Vec<i16> {
    (0..SAMPLES)
        .map(|index| i16::try_from(index.checked_rem(1000).unwrap_or(0)).unwrap())
        .collect()
}

/// A mono 16-bit WAV file of `samples` at [`RATE`], laid out as Python's `wave` module writes one.
#[expect(clippy::little_endian_bytes, reason = "a WAV file is little-endian")]
fn wav(samples: &[i16]) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 2).unwrap();
    let rate = u32::from(RATE);
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    bytes.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
    bytes
}

/// The width and height a JPEG's start-of-frame segment gives it.
///
/// # Errors
///
/// When the bytes end before a start-of-frame segment does.
#[expect(
    clippy::big_endian_bytes,
    clippy::single_call_fn,
    reason = "a JPEG's segment lengths and sizes are big-endian; the parse is named apart from the check it serves"
)]
fn jpeg_size(bytes: &[u8]) -> Result<(u16, u16), DeviceError> {
    let word = |offset: usize| {
        bytes
            .get(offset..offset + 2)
            .and_then(|pair| <[u8; 2]>::try_from(pair).ok())
            .map(u16::from_be_bytes)
            .ok_or_else(|| DeviceError::Malformed {
                key: FRAME_KEY,
                problem: format!("ends at {} bytes, inside a segment", bytes.len()),
            })
    };
    let mut offset = 2;
    loop {
        let marker = word(offset)?;
        // SOF0 to SOF2: baseline, extended and progressive, each holding the frame's height and then its width.
        if (0xFFC0..=0xFFC2).contains(&marker) {
            return Ok((word(offset + 7)?, word(offset + 5)?));
        }
        offset += 2 + usize::from(word(offset + 2)?);
    }
}
