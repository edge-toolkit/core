//! What the simulated devices share: a serial port bridged over TCP for a runner to lend, the wait for the module that
//! opens it, and the wait for what that module stores.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// How long a simulated device waits for its module to connect, and then for the module to send it anything.
pub const PATIENCE: Duration = Duration::from_mins(1);

/// How often a simulated device looks for its module's connection, and a wait for what it stored looks again.
const POLL: Duration = Duration::from_millis(50);

/// Why a simulated device's check of its module failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DeviceError {
    /// The hub's storage directory could not be read.
    #[error("reading the hub's storage at {}: {source}", dir.display())]
    Storage {
        /// The storage directory.
        dir: PathBuf,
        /// Why it could not be read.
        source: io::Error,
    },
    /// No bucket held what the module should have stored by the time the wait ran out.
    #[error("no {key} under {} is the one expected; found {found:?}", dir.display())]
    NotStored {
        /// The key the module stores under.
        key: String,
        /// The storage directory.
        dir: PathBuf,
        /// The start of what each bucket held under `key` instead.
        found: Vec<String>,
    },
    /// The module sent the device something other than the command it should have.
    #[error("the module sent {sent:02X?} rather than {expected:02X?}")]
    WrongCommand {
        /// What the module sent.
        sent: Vec<u8>,
        /// The command it should have sent.
        expected: Vec<u8>,
    },
    /// A stored file was not of the kind expected.
    #[error("the stored {key} {problem}")]
    Malformed {
        /// The key it was stored under.
        key: &'static str,
        /// What is wrong with it.
        problem: String,
    },
    /// The thread serving a simulated device panicked.
    #[error("the simulated device's thread panicked")]
    Panicked,
}

/// A listener for a simulated device's one connection, and the device a runner lends it as, `tcp://127.0.0.1:<port>`.
#[must_use]
pub fn bridge() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let device = format!("tcp://{}", listener.local_addr().unwrap());
    (listener, device)
}

/// The connection a module makes to `listener` within [`PATIENCE`].
///
/// Polled rather than blocking, so a module that never connects leaves the simulated device to end, and the test
/// waiting on it to fail, rather than to hang.
///
/// # Errors
///
/// `TimedOut` when no module connects in time.
pub fn accept(listener: &TcpListener) -> io::Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if let Ok((connection, _)) = listener.accept() {
            connection.set_nonblocking(false)?;
            return Ok(connection);
        }
        thread::sleep(POLL);
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "no module connected to the simulated device",
    ))
}

/// What a module stored as `key` in any bucket under `storage_dir`, once `accepted` takes it.
///
/// Waits up to `budget`, since a module stores as it reads and a runner may still be storing when its device stops.
///
/// # Errors
///
/// [`DeviceError::NotStored`] when no bucket holds a `key` that `accepted` takes by then, saying what each one holds
/// instead, and [`DeviceError::Storage`] when the storage directory cannot be read.
pub fn wait_for_stored<F>(storage_dir: &Path, key: &str, budget: Duration, accepted: F) -> Result<Vec<u8>, DeviceError>
where
    F: Fn(&[u8]) -> bool,
{
    let deadline = Instant::now() + budget;
    loop {
        let buckets = match std::fs::read_dir(storage_dir) {
            Ok(buckets) => buckets,
            Err(source) => {
                return Err(DeviceError::Storage {
                    dir: storage_dir.to_path_buf(),
                    source,
                });
            }
        };
        let stored: Vec<Vec<u8>> = buckets
            .filter_map(Result::ok)
            .filter_map(|bucket| fs_err::read(bucket.path().join(key)).ok())
            .collect();
        if let Some(found) = stored.iter().find(|bytes| accepted(bytes)) {
            return Ok(found.clone());
        }
        if Instant::now() >= deadline {
            return Err(DeviceError::NotStored {
                key: key.to_owned(),
                dir: storage_dir.to_path_buf(),
                found: stored
                    .iter()
                    .map(|bytes| String::from_utf8_lossy(bytes.get(..200).unwrap_or(bytes)).into_owned())
                    .collect(),
            });
        }
        thread::sleep(POLL);
    }
}
