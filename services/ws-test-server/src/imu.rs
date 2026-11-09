//! A simulated `MentorPi` controller board, Hiwonder's RRC Lite, which a test lends a runner as a serial port bridged
//! over TCP, and the check of the IMU reading a module stores from it.
//!
//! The simulated board accepts one connection and sends IMU reports, among a corrupt one and a report of another kind,
//! then holds the connection open until the module hangs up, as a board on a serial port never closes.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::device::{self, DeviceError, PATIENCE};

/// The key an IMU module stores the latest reading under, in its agent's bucket.
pub const READING_KEY: &str = "imu-reading.json";

/// The function code of a frame reporting a button press.
const FUNCTION_KEY: u8 = 6;

/// The function code of a frame reporting an IMU reading.
pub const FUNCTION_IMU: u8 = 7;

/// Standard gravity, of which the board reports acceleration in multiples.
const STANDARD_GRAVITY: f64 = 9.806_65;

/// The last reading the simulated board reports: acceleration in g, then rotation in degrees a second, along x, y, z.
const LAST: [f32; 6] = [0.5, -0.25, 1.0, 90.0, -45.0, 180.0];

/// A simulated controller board, waiting for a module to open it.
///
/// Its thread is never joined: it ends when the runner the test stops hangs up, after the check is done.
#[non_exhaustive]
pub struct SimulatedController {
    /// The device a runner lends it as.
    device: String,
}

impl SimulatedController {
    /// The device a runner lends the board as, `tcp://127.0.0.1:<port>`, for its `SERIAL_PORTS_<NAME>`.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// A board that reports two IMU readings, with a corrupt reading and a button press between them.
    ///
    /// The last reading is unlike the first, so a module that stored the earlier one is caught, and it must pass over
    /// the two between.
    #[must_use]
    pub fn start() -> Self {
        let mut stream = frame(FUNCTION_IMU, &imu_data([0.0, 0.0, 1.0, 0.0, 0.0, 0.0]));
        let mut corrupt = frame(FUNCTION_IMU, &imu_data([9.0; 6]));
        *corrupt.last_mut().unwrap() ^= 1;
        stream.extend(corrupt);
        stream.extend(frame(FUNCTION_KEY, &[1, 0]));
        stream.extend(frame(FUNCTION_IMU, &imu_data(LAST)));
        let (listener, device) = device::bridge();
        let _serving: JoinHandle<()> = thread::spawn(move || serve(&listener, &stream));
        Self { device }
    }

    /// Check the module stored the board's last reading under `storage_dir` within `budget`, in SI units.
    ///
    /// # Errors
    ///
    /// When it did not, saying what it stored instead, so a test can stop the runner it started before it fails.
    pub fn verify(self, storage_dir: &Path, budget: Duration) -> Result<(), DeviceError> {
        let expected = in_si_units(LAST);
        let accepted = |bytes: &[u8]| {
            serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|reading| matches_si(&reading, &expected))
        };
        device::wait_for_stored(storage_dir, READING_KEY, budget, accepted).map(drop)
    }
}

/// Serve `stream` to the one module that connects to `listener`, then hold the connection until it hangs up.
#[expect(
    clippy::single_call_fn,
    reason = "the simulated board's thread, named apart from its start"
)]
fn serve(listener: &TcpListener, stream: &[u8]) {
    let Ok(mut connection) = device::accept(listener) else {
        return;
    };
    connection.write_all(stream).unwrap();
    connection.set_read_timeout(Some(PATIENCE)).unwrap();
    let mut ignored = Vec::new();
    let _hung_up: std::io::Result<usize> = connection.read_to_end(&mut ignored);
}

/// The bytes the board sends for a frame of `function` carrying `data`.
///
/// `AA 55`, the code, the length, the data, and the CRC-8 of the code, length and data.
#[must_use]
pub fn frame(function: u8, data: &[u8]) -> Vec<u8> {
    let mut checked = vec![function, u8::try_from(data.len()).unwrap()];
    checked.extend_from_slice(data);
    let mut bytes = vec![0xAA, 0x55];
    bytes.extend_from_slice(&checked);
    bytes.push(crc8(&checked));
    bytes
}

/// The Dallas/Maxim CRC-8 of `bytes`, which the board checks each frame with.
#[must_use]
pub fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0_u8, |crc, &byte| {
        (0_u8..8).fold(crc ^ byte, |crc, _| {
            if crc & 1_u8 == 1 {
                (crc >> 1_u32) ^ 0x8C_u8
            } else {
                crc >> 1_u32
            }
        })
    })
}

/// The data of an IMU frame: six little-endian floats.
#[expect(clippy::little_endian_bytes, reason = "the board's protocol is little-endian")]
fn imu_data(values: [f32; 6]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

/// A reading in the SI units a module stores: acceleration in metres per second squared, rotation in radians a second.
#[expect(
    clippy::float_arithmetic,
    clippy::single_call_fn,
    reason = "unit conversion of the board's float readings, named for what it converts to"
)]
fn in_si_units(reading: [f32; 6]) -> [f64; 6] {
    let [accel_x, accel_y, accel_z, turn_x, turn_y, turn_z] = reading.map(f64::from);
    [
        accel_x * STANDARD_GRAVITY,
        accel_y * STANDARD_GRAVITY,
        accel_z * STANDARD_GRAVITY,
        turn_x.to_radians(),
        turn_y.to_radians(),
        turn_z.to_radians(),
    ]
}

/// Whether a stored reading's acceleration and rotation are `expected`, to within a rounding.
#[expect(
    clippy::float_arithmetic,
    clippy::single_call_fn,
    reason = "comparing floats to within a rounding, named apart from the wait it decides"
)]
fn matches_si(reading: &serde_json::Value, expected: &[f64; 6]) -> bool {
    let stored: Vec<f64> = ["acceleration_m_s2", "angular_velocity_rad_s"]
        .iter()
        .filter_map(|field| reading.get(field).and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(serde_json::Value::as_f64)
        .collect();
    stored.len() == expected.len()
        && stored
            .iter()
            .zip(expected)
            .all(|(stored, expected)| (stored - expected).abs() < 1e-9_f64)
}
