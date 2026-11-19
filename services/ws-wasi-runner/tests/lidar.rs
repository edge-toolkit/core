//! Runs wasi-lidar1 through et-ws-wasi-runner against an in-process ws-server and a simulated lidar.
#![cfg(test)]

use command_error::CommandExt as _;
use edge_toolkit::config::{Language, mise_env_includes};

/// Run wasi-lidar1 against a simulated COIN-D6 lent as a port bridged over TCP.
///
/// The module asks the lidar to scan, stores each revolution it receives, and ends when the lidar hangs up, which
/// leaves the last whole revolution in its bucket.
#[test]
fn lidar1_stores_the_latest_revolution() {
    if !mise_env_includes(Language::Rust) {
        return;
    }
    let server = et_ws_test_server::start();
    let lidar = et_ws_test_server::lidar::SimulatedLidar::start();
    let bin = env!("CARGO_BIN_EXE_et-ws-wasi-runner");
    let module = "et-ws-wasi-lidar1";
    let _: std::process::ExitStatus = std::process::Command::new(bin)
        .env("RUNNER_MODULE", format!("{}{module}", et_org::NPM_SCOPE))
        .env("WS_SERVER_URL", &server.ws_url)
        .env("SERIAL_PORTS_LIDAR", lidar.device())
        .env("ET_TEST_WS_WASI_RUNNER_FAST_EXIT", "1")
        .status_checked()
        .unwrap_or_else(|error| panic!("{module} runner failed: {error}"));
    lidar
        .verify(server.storage_dir.path(), std::time::Duration::ZERO)
        .unwrap();
}
