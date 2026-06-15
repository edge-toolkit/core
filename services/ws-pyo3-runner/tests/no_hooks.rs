//! Verify the load-time sanity check: a module defining none of the runner
//! hooks must fail to load rather than connect and sit idle. The import
//! happens in `initialize`, before any connection, so the runner exits
//! non-zero without needing a server.

#![cfg(test)]

use std::error::Error;
use std::process::Command;

#[test]
fn module_without_hooks_fails_to_load() -> Result<(), Box<dyn Error>> {
    let py_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let output = Command::new(bin)
        .env("RUNNER_MODULE", "no_hooks")
        .env("PYO3_PYTHONPATH", &py_path)
        // Safety net: if the check ever regressed and import succeeded, this
        // bounds the otherwise-forever connect retry so the test fails (on the
        // assertion below) instead of hanging.
        .env("RUNNER_TIMEOUT", "10s")
        .env("RUST_LOG", "error")
        .output()?;

    if output.status.success() {
        return Err(format!("a hookless module must fail to load; got {:?}", output.status).into());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("none of the runner hooks") {
        return Err(format!("stderr should explain the missing hooks; got: {stderr}").into());
    }
    Ok(())
}
