//! A throwaway `nats-server` with `JetStream` enabled, for tests.
//!
//! Each [`NatsServer`] is its own process on its own loopback port with its own `JetStream` store directory, so tests
//! running in parallel never share a stream or a KV bucket. The binary is the `nats-server` mise tool; a missing one
//! panics at spawn, because it means the environment skipped `mise install`.
#![expect(
    clippy::unwrap_used,
    reason = "test support; an unspawnable broker or an unwritable temp dir means an unusable environment"
)]

use std::process::{Command, Stdio};

use command_error::CommandExt as _;
use et_test_helpers::{ChildGuard, reserve_port, wait_for_port};
use tempfile::TempDir;

/// A running `nats-server`, killed and its store deleted on drop.
#[non_exhaustive]
pub struct NatsServer {
    // Declared before `_store`, which is held only for its drop, so the process is gone before its directory is.
    child: ChildGuard,
    _store: TempDir,
    port: u16,
}

impl NatsServer {
    /// Start a broker with `JetStream` enabled on a free loopback port, returning once it accepts connections.
    #[must_use]
    pub fn start() -> Self {
        let port = reserve_port();
        let store = TempDir::new().unwrap();
        let child = Command::new("nats-server")
            .args([
                "--jetstream",
                "--addr",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--store_dir",
            ])
            .arg(store.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn_checked()
            .unwrap()
            .into_child();
        let child = ChildGuard::new(child);
        // Not an `assert!`: its failure arm would be a branch no test can take, since the broker fails to listen only
        // in a broken environment, and `libs/` is held to every branch. The panic stays in `unwrap`, outside the crate.
        wait_for_port(port).then_some(()).unwrap();
        Self {
            child,
            _store: store,
            port,
        }
    }

    /// The `nats://` URL clients connect to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.port)
    }

    /// Whether the broker process is still running.
    pub fn is_running(&mut self) -> bool {
        !self.child.has_exited()
    }
}
