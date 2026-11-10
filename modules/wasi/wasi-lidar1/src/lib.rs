//! Reads the COIN-D6 lidar on Hiwonder's `MentorPi` robots, keeping its latest revolution in the agent's storage.
//!
//! Opens the serial port the runner lends as `lidar`, asks the lidar to scan, and decodes what it sends with
//! `et-lidar`. Each whole revolution replaces `lidar-scan.json` in the agent's own bucket, where another agent, or
//! anything that can reach the hub's REST API, reads the latest. It holds the scan frequency in hertz, and one array
//! per sample field, in the order the lidar took them: the angle in degrees clockwise from the lidar's zero, the
//! distance in millimetres (0 when nothing came back), the intensity, and the high-reflectivity flag. The module
//! runs until the port closes, which a lidar on a serial port never does, so a runner bounds it with its timeout.

#![cfg(target_os = "wasi")]
#![cfg_attr(feature = "docs", doc = "## Feature flags", doc = document_features::document_features!())]

use et_lidar::coin_d6::{BAUD_RATE, START_SCAN, Scan, Scanner};
use et_wasi_guest::et::ws_wasi::serial::{self, SerialError};
use et_wasi_guest::et::ws_wasi::ws::disconnect;
use et_wasi_guest::exports::et::ws_wasi::entry::{EntryError, Guest};
use et_wasi_guest::{info, start};
use serde_json::json;

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");

/// The name the runner lends the lidar's port under.
const PORT: &str = "lidar";

/// The key the latest revolution is stored under, in the agent's own bucket.
const SCAN_KEY: &str = "lidar-scan.json";

/// The most bytes one read takes, about a fifth of a second of the lidar's stream.
const READ_MAX: u32 = 4096;

/// How long one read waits for the lidar to send anything.
const READ_TIMEOUT_MS: u32 = 100;

struct Component;

impl Guest for Component {
    /// Store each revolution the lidar sends in this agent's bucket, until its port closes.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "entry.run is an async export of the WIT world, so the generated trait fixes the signature"
    )]
    async fn run() -> Result<(), EntryError> {
        let bucket = et_wasi_guest::wasi::keyvalue::store::open(&start(LOG_CONTEXT)?)?;
        let port = serial::open(PORT, BAUD_RATE)?;
        port.write(&START_SCAN)?;
        info(&format!("asked the lidar on port {PORT} to scan"));

        let mut scanner = Scanner::default();
        let mut stored = 0_u64;
        loop {
            let bytes = match port.read(READ_MAX, READ_TIMEOUT_MS) {
                Ok(bytes) => bytes,
                Err(SerialError::Closed) => break,
                Err(error) => return Err(error.into()),
            };
            for scan in scanner.push(&bytes) {
                bucket.set(SCAN_KEY, scan_json(&scan).as_bytes())?;
                if stored == 0 {
                    info(&format!(
                        "stored the first revolution to {SCAN_KEY}: {} samples at {} Hz",
                        scan.samples.len(),
                        scan.frequency_hz()
                    ));
                }
                stored = stored.saturating_add(1);
            }
        }

        let decoder = scanner.decoder();
        info(&format!(
            "the lidar's port closed after {stored} revolutions, with {} bytes skipped and {} packets rejected",
            decoder.skipped_bytes(),
            decoder.rejected_packets()
        ));
        disconnect();
        #[cfg(feature = "coverage")]
        et_wasi_guest::dump_coverage("et_ws_wasi_lidar1.profraw");
        Ok(())
    }
}

/// The JSON a revolution is stored as.
#[expect(
    clippy::single_call_fn,
    reason = "the stored form of a revolution, named apart from the read loop"
)]
fn scan_json(scan: &Scan) -> String {
    let samples = &scan.samples;
    json!({
        "frequency_hz": scan.frequency_hz(),
        "angle_deg": samples.iter().map(|sample| sample.degrees()).collect::<Vec<_>>(),
        "distance_mm": samples.iter().map(|sample| sample.distance_mm).collect::<Vec<_>>(),
        "intensity": samples.iter().map(|sample| sample.intensity).collect::<Vec<_>>(),
        "high_reflectivity": samples.iter().map(|sample| sample.high_reflectivity).collect::<Vec<_>>(),
    })
    .to_string()
}

et_wasi_guest::export!(Component);
