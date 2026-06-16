//! Integration tests: start a ws-server in-process and run each WASI module
//! via et-ws-wasi-runner. Mirror of `services/ws-worker/tests/modules.rs`'s
//! removed predecessor -- same shape, but the spawned binary runs WASI
//! components rather than browser-targeted JS.

#![cfg(test)]
#![expect(
    clippy::expect_used,
    reason = "test code: process spawn failure should fail the test"
)]

use rstest::rstest;

// Skipped on Windows: the wasi runner gets a 404 fetching the module's
// `pkg/package.json` because `build-modules` likely isn't producing it
// under mise's cmd.exe default shell. Re-enable once the Windows task-shell
// story is sorted.
#[rstest]
#[case::wasi_comm1("et-ws-wasi-comm1")]
#[case::wasi_data1("et-ws-wasi-data1")]
#[case::wasi_graphics_info("et-ws-wasi-graphics-info")]
#[cfg_attr(windows, ignore = "pkg/package.json 404 on Windows -- see comment above")]
fn module_runs_successfully(#[case] module: &str) {
    let server = et_ws_test_server::start();

    let bin = env!("CARGO_BIN_EXE_et-ws-wasi-runner");
    // ET_TEST_WS_WASI_RUNNER_FAST_EXIT: opt the spawned runner into its macOS-only
    // exit(0) short-circuit so ORT 1.22's libc++ teardown race doesn't surface
    // as a None exit code (see main.rs for the exact stderr signature).
    // No-op on Linux/Windows.
    let status = std::process::Command::new(bin)
        .env("RUNNER_MODULE", module)
        .env("WS_SERVER_URL", &server.ws_url)
        .env("ET_TEST_WS_WASI_RUNNER_FAST_EXIT", "1")
        .status()
        .expect("failed to spawn et-ws-wasi-runner");

    assert!(status.success(), "{module} exited with code {:?}", status.code());
}
