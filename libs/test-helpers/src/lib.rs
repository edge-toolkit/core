//! Low-dependency helpers shared across the workspace's integration tests.
//!
//! Only genuinely reusable, low-dependency utilities belong here (see CLAUDE.md). Heavier or
//! domain-specific fixtures live in their own test-support crate instead -- e.g. `et-ws-test-server`
//! (an in-process ws-server) or `et-test-otlp` (OTLP emit + capture-assertion support).
#![expect(
    clippy::unwrap_used,
    reason = "test helper: a missing free port or unpiped child stderr should fail the test loudly"
)]

use std::io::Read as _;
use std::process::Child;
use std::sync::{Arc, Mutex};

use retry::delay::Fixed;
use retry::retry;
/// The workspace's canonical fake-environment test harness, re-exported for every test crate.
///
/// `temp-env` runs a closure with environment variables temporarily set or unset and restores them afterward,
/// serialised by its own global lock. Prefer it (e.g. `et_test_helpers::temp_env::with_var("KEY", Some("v"),
/// || ...)`) over touching `std::env` by hand -- `set_var`/`remove_var` are `unsafe` on edition 2024 and race
/// other threads. For the common "make a `PATH`-resolved tool look absent" case, use [`with_empty_path`].
pub use temp_env;

/// Run `run` with `PATH` emptied so a bare-name `Command` spawn fails, restoring `PATH` afterward.
///
/// Exercises "tool not found on `PATH`" fallbacks deterministically: the OS-level program resolution inside
/// `Command::new("<tool>")` sees the empty `PATH` and fails to find the binary. Thin wrapper over
/// [`temp_env::with_var`]; returns whatever `run` returns.
pub fn with_empty_path<Out, Body>(run: Body) -> Out
where
    Body: FnOnce() -> Out,
{
    temp_env::with_var("PATH", Some(""), run)
}

/// Reserve a free loopback TCP port, bound then released for the caller to claim.
///
/// Same bind-`:0`-then-drop trick every test-support crate used to hand-roll; there is an inherent
/// race (another process could grab the port before the caller binds it) that is acceptable in tests.
#[must_use]
pub fn reserve_port() -> u16 {
    port_check::free_local_port().unwrap()
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
        // Best-effort teardown, also invoked from Drop -- no caller to propagate to, so discard.
        let _kill = self.child.kill();
        let _wait = self.child.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Drain a child's piped stderr into a shared buffer on a background thread.
///
/// The buffer is populated once the child's stderr reaches EOF (i.e. it exits), so read it after shutting
/// the child down. The child must have been spawned with `Stdio::piped()` on stderr. Use this for daemons
/// that log to stderr (e.g. vector); use [`drain_stdout`] for those that log to stdout (e.g. openobserve).
#[must_use]
pub fn drain_stderr(child: &mut Child) -> Arc<Mutex<String>> {
    drain_pipe(child.stderr.take().unwrap())
}

/// Drain a child's piped stdout into a shared buffer on a background thread.
///
/// The stdout counterpart of [`drain_stderr`], for daemons that log to stdout. Same EOF-on-exit semantics;
/// the child must have been spawned with `Stdio::piped()` on stdout.
#[must_use]
pub fn drain_stdout(child: &mut Child) -> Arc<Mutex<String>> {
    drain_pipe(child.stdout.take().unwrap())
}

/// Spawn a detached thread that reads `pipe` to EOF into a shared string buffer.
///
/// Reading in a thread avoids the deadlock where the child fills its output pipe while the test is blocked
/// elsewhere. Nothing joins the handle, so neither the read result nor the thread's own outcome has a caller
/// to propagate to -- both are intentionally discarded (partial output on a read error is still worth keeping
/// for diagnostics).
fn drain_pipe<Pipe>(pipe: Pipe) -> Arc<Mutex<String>>
where
    Pipe: std::io::Read + Send + 'static,
{
    let log = Arc::new(Mutex::new(String::default()));
    let sink = Arc::clone(&log);
    let _drainer = std::thread::spawn(move || {
        let mut buffer = String::default();
        let _read = std::io::BufReader::new(pipe).read_to_string(&mut buffer);
        *sink.lock().unwrap() = buffer;
    });
    log
}
