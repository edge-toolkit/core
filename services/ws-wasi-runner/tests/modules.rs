//! Integration tests: start a ws-server in-process and run each WASI module
//! via et-ws-wasi-runner. Mirror of `services/ws-worker/tests/modules.rs`'s
//! removed predecessor — same shape, but the spawned binary runs WASI
//! components rather than browser-targeted JS.
use rstest::rstest;

#[rstest]
#[case::wasi_graphics_info("et-ws-wasi-graphics-info")]
fn module_runs_successfully(#[case] module: &str) {
    let server = et_ws_test_server::start();

    let bin = env!("CARGO_BIN_EXE_et-ws-wasi-runner");
    let status = std::process::Command::new(bin)
        .env("RUNNER_MODULE", module)
        .env("WS_SERVER_URL", &server.ws_url)
        .status()
        .expect("failed to spawn et-ws-wasi-runner");

    assert!(status.success(), "{module} exited with code {:?}", status.code());
}
