//! A simulated COIN-D6 lidar, which a test lends a runner as a serial port bridged over TCP, and the check of what a
//! lidar module stores from it, shared by every runner's lidar test.
//!
//! The simulated lidar accepts one connection and reads the command the module sends, then streams its revolutions,
//! and a packet starting one more so that the last of them is whole, and hangs up. A module reading it sees the port
//! close once it has had every revolution.

use std::io::{Read as _, Write as _};
use std::net::{Shutdown, TcpListener};
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use et_lidar::coin_d6::{FULL_TURN, Packet, START_SCAN, Sample};

use crate::device::{self, DeviceError, PATIENCE};

/// The scan frequency every simulated revolution reports, in tenths of a hertz.
const FREQUENCY_DHZ: u8 = 100;

/// The key a lidar module stores the latest revolution under, in its agent's bucket.
pub const SCAN_KEY: &str = "lidar-scan.json";

/// How many samples a simulated packet holds after a revolution's first, as many as a COIN-D6 sends.
const SAMPLES_PER_PACKET: usize = 25;

/// How many samples each simulated revolution holds, as many as a COIN-D6 takes in one at 10 Hz.
const SAMPLES_PER_REVOLUTION: u16 = 400;

/// A simulated COIN-D6, waiting for a module to open it.
#[non_exhaustive]
pub struct SimulatedLidar {
    /// The device a runner lends it as.
    device: String,
    /// The distances of the last revolution it sends, which is what a module should store last.
    last: Vec<u16>,
    /// The thread serving the lidar, which ends with the command the module sent.
    serving: JoinHandle<Vec<u8>>,
}

impl SimulatedLidar {
    /// The device a runner lends the lidar as, `tcp://127.0.0.1:<port>`, for its `SERIAL_PORTS_<NAME>`.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// A lidar that sends three revolutions, the last unlike the two before it.
    ///
    /// A module that stored an earlier one is then caught.
    #[must_use]
    pub fn start() -> Self {
        let last: Vec<u16> = (500..500 + SAMPLES_PER_REVOLUTION).collect();
        let flat = |distance| vec![distance; usize::from(SAMPLES_PER_REVOLUTION)];
        let mut stream: Vec<u8> = [flat(1000), flat(2000), last.clone()]
            .iter()
            .flat_map(|distances| revolution(distances))
            .collect();
        stream.extend(revolution(&[0]));
        let (listener, device) = device::bridge();
        let serving = thread::spawn(move || serve(&listener, &stream));
        Self { device, last, serving }
    }

    /// Check the module asked the lidar to scan, and stored its last revolution under `storage_dir` within `budget`.
    ///
    /// # Errors
    ///
    /// When it did not, saying what it did instead, so a test can stop the runner it started before it fails.
    pub fn verify(self, storage_dir: &Path, budget: Duration) -> Result<(), DeviceError> {
        let Ok(command) = self.serving.join() else {
            return Err(DeviceError::Panicked);
        };
        if command != START_SCAN {
            return Err(DeviceError::WrongCommand {
                sent: command,
                expected: START_SCAN.to_vec(),
            });
        }
        let distances = serde_json::json!(self.last);
        // FREQUENCY_DHZ's 100 tenths, in the hertz a module stores, and the first sample at the lidar's zero.
        let frequency = serde_json::json!(10.0_f64);
        let zero = serde_json::json!(0.0_f64);
        let accepted = |bytes: &[u8]| {
            serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|scan| {
                scan.get("distance_mm") == Some(&distances)
                    && scan.get("frequency_hz") == Some(&frequency)
                    && scan.get("angle_deg").and_then(|angles| angles.get(0)) == Some(&zero)
            })
        };
        device::wait_for_stored(storage_dir, SCAN_KEY, budget, accepted).map(drop)
    }
}

/// Serve `stream` to the one module that connects to `listener`, returning the command it sent first.
#[expect(
    clippy::single_call_fn,
    reason = "the simulated lidar's thread, named apart from its start"
)]
fn serve(listener: &TcpListener, stream: &[u8]) -> Vec<u8> {
    let Ok(mut connection) = device::accept(listener) else {
        return Vec::new();
    };
    connection.set_read_timeout(Some(PATIENCE)).unwrap();
    let mut command = vec![0; START_SCAN.len()];
    if connection.read_exact(&mut command).is_err() {
        return Vec::new();
    }
    connection.write_all(stream).unwrap();
    connection.shutdown(Shutdown::Both).unwrap();
    command
}

/// The bytes a COIN-D6 sends for one revolution of samples `distances` away, spaced evenly round a turn from its zero.
#[must_use]
pub fn revolution(distances: &[u16]) -> Vec<u8> {
    let count = distances.len();
    let samples: Vec<Sample> = distances
        .iter()
        .enumerate()
        .map(|(index, &distance)| {
            let angle = (index * usize::from(FULL_TURN)).checked_div(count).unwrap();
            Sample::new(u16::try_from(angle).unwrap(), distance, 100, false)
        })
        .collect();
    let (first, rest) = samples.split_first().unwrap();
    let mut bytes = Packet::new(true, FREQUENCY_DHZ, vec![*first]).encode().unwrap();
    for chunk in rest.chunks(SAMPLES_PER_PACKET) {
        bytes.extend(Packet::new(false, 0, chunk.to_vec()).encode().unwrap());
    }
    bytes
}
