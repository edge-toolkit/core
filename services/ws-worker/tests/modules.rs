/// Integration tests: start a ws-server in-process and run each module via et-ws-worker.
use rstest::rstest;

#[rstest]
#[case::data1("et-ws-data1")]
#[case::graphics_info("et-ws-graphics-info")]
#[case::pydata1("et-ws-pydata1")]
fn module_runs_successfully(#[case] module: &str) {
    let server = et_ws_test_server::start();

    let bin = env!("CARGO_BIN_EXE_et-ws-worker");
    let status = std::process::Command::new(bin)
        .env("WORKER_MODULE", module)
        .env("WS_SERVER_URL", &server.ws_url)
        .status()
        .expect("failed to spawn et-ws-worker");

    assert!(status.success(), "{module} exited with code {:?}", status.code());
}
