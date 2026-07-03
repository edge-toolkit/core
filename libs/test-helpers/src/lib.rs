//! Low-dependency helpers shared across the workspace's integration tests.
//!
//! Only genuinely reusable, low-dependency utilities belong here (see CLAUDE.md). Heavier or
//! domain-specific fixtures live in their own test-support crate instead -- e.g. `et-ws-test-server`
//! (an in-process ws-server) or `int-otlp-mock` (a mock OTLP collector).
#![expect(
    clippy::expect_used,
    reason = "test helper: a missing free port or unpiped child stderr should fail the test loudly"
)]

use std::io::Read as _;
use std::process::Child;
use std::sync::{Arc, Mutex};

use retry::delay::Fixed;
use retry::retry;

/// Reserve a free loopback TCP port, bound then released for the caller to claim.
///
/// Same bind-`:0`-then-drop trick every test-support crate used to hand-roll; there is an inherent
/// race (another process could grab the port before the caller binds it) that is acceptable in tests.
#[must_use]
pub fn reserve_port() -> u16 {
    port_check::free_local_port().expect("no free local port")
}

/// Wait until `port` accepts a TCP connection, polling ~every 100ms for up to ~20s.
///
/// Returns `false` if the deadline passes without the port coming up.
#[must_use]
pub fn wait_for_port(port: u16) -> bool {
    retry(Fixed::from_millis(100).take(200), || {
        port_check::is_port_reachable(("127.0.0.1", port))
            .then_some(())
            .ok_or(())
    })
    .is_ok()
}

/// Owns a spawned child process and kills + reaps it on drop, so a panicking test never leaks it.
#[non_exhaustive]
pub struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    /// Take ownership of an already-spawned `child`.
    #[must_use]
    pub const fn new(child: Child) -> Self {
        Self { child }
    }

    /// Kill the child and reap it now (also runs on drop; errors are ignored).
    pub fn shutdown(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Drain a child's piped stderr into a shared buffer on a background thread.
///
/// Reading in a thread avoids the deadlock where the child fills its stderr pipe while the test is
/// blocked elsewhere. The buffer is populated once the child's stderr reaches EOF (i.e. it exits),
/// so read it after shutting the child down. The child must have been spawned with `Stdio::piped()`.
#[must_use]
pub fn drain_stderr(child: &mut Child) -> Arc<Mutex<String>> {
    let stderr = child.stderr.take().expect("child stderr was not piped");
    let log = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&log);
    drop(std::thread::spawn(move || {
        let mut buffer = String::new();
        drop(std::io::BufReader::new(stderr).read_to_string(&mut buffer));
        *sink.lock().expect("stderr log mutex") = buffer;
    }));
    log
}
