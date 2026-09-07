//! Wait until the hub serves a module, for a generated deployment's runner to gate on.
//!
//! A generated deployment starts the hub and its runners together, and a runner resolves its module by fetching
//! `/modules/<name>/package.json` over HTTP -- a request that fails outright rather than retrying. Without a gate
//! the scenario is a coin toss, so each runner task waits here first.
//!
//! What it polls is the module's own URL rather than the hub's `/health`. Readiness means "can serve this
//! module", which is strictly stronger than "is listening": the hub binds its port before it has finished
//! scanning the module paths, so a health check can pass while the very next request 404s.
//!
//! This lives in `et-cli` rather than being a shell loop around an HTTP client because the generated task then
//! needs no tool beyond the `cargo` it already uses. An earlier version polled with `xh`, which the generated
//! config declared in its own `[tools]` -- but `task.run_auto_install` is off, so nothing installed it, and it is
//! pinned only in the maintainer-only env. The task looked fine and silently spun out its whole timeout wherever
//! that tool was absent, CI included.

use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::error::CliError;
use crate::hub_http_base;

/// Gap between polls, short enough to add no noticeable delay once the hub is up.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Per-request timeout, so a hub that accepts but never answers cannot stall the whole wait.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Block until the hub serves `module`, or `timeout` elapses.
///
/// Returns the URL that answered, for the caller to report.
pub fn wait_for_module(module: &str, timeout: Duration) -> Result<String, CliError> {
    let url = format!("{}/modules/{module}/package.json", hub_http_base());
    // `?` rather than a `map_err`: `CliError::HubProbe` carries `#[from] reqwest::Error`, which is the
    // conversion the repo's no-map-err rule asks for.
    let client = reqwest::blocking::Client::builder().timeout(REQUEST_TIMEOUT).build()?;

    let start = Instant::now();
    // Elapsed-versus-timeout rather than a computed deadline, so no arithmetic on an `Instant` is needed.
    while start.elapsed() < timeout {
        if let Ok(response) = client.get(&url).send()
            && response.status().is_success()
        {
            return Ok(url);
        }
        sleep(POLL_INTERVAL);
    }

    Err(CliError::HubNotReady {
        url,
        seconds: timeout.as_secs(),
    })
}
