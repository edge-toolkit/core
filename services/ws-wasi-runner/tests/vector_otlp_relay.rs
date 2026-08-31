//! Offline store-and-forward relay proof, using the real Vector binary and a real `OpenTelemetry` exporter.
//!
//! The runners must keep working while offline: telemetry has to be buffered locally and forwarded once the
//! backend reappears. This test proves an off-the-shelf OSS relay (Vector's `opentelemetry` source -> disk
//! buffer -> `opentelemetry` sink) does exactly that, end to end:
//!
//!   1. Reserve a free port for the collector, but do NOT start it -- the backend is "offline".
//!   2. Start `vector` with its sink pointed at that dead port and a disk buffer, so accepted events are
//!      stored, not dropped.
//!   3. Emit one span through the OpenTelemetry SDK's OTLP/HTTP exporter (the same `opentelemetry-otlp`
//!      transport `et-otlp` uses) into Vector's source. Vector accepts it (200) and buffers it, since the
//!      sink target is down.
//!   4. Bring the mock collector up on the reserved port ("back online").
//!   5. Assert the buffered span is forwarded to the mock, intact -- same span name and service.
//!
//! The Vector config is the static, checked-in `config/vector-otlp-relay.yaml`; per-run ports and the
//! buffer's `data_dir` are passed as `RELAY_*` env vars, which Vector interpolates.

#![cfg(test)]
#![expect(clippy::single_call_fn, reason = "test code: named single-use step helpers")]

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use command_error::CommandExt as _;
use et_test_helpers::{ChildGuard, drain_stderr, reserve_port, wait_for_port};
use et_test_otlp::{Protocol, ServerHandle};
use tokio::runtime::Runtime;

const SERVICE_NAME: &str = "vector-relay-test";
const SPAN_NAME: &str = "relay-probe";

/// Redelivery ceiling, matching the `retry`-based poll this replaced.
///
/// Store-and-forward redelivery is inherently latent: Vector retries the initially-dead sink with an exponential
/// backoff (`retry_initial_backoff_secs=1`, doubling), so when its first attempts race the mock's listener coming
/// up, the next retry can land tens of seconds later. The old 30s ceiling intermittently timed that out on cold
/// CI runners; the wait returns the instant the span lands, so the wider ceiling costs nothing on the happy path.
const RELAY_TIMEOUT: Duration = Duration::from_mins(2);

#[test]
fn vector_relays_buffered_otlp_after_backend_comes_online() {
    // 1. Reserve the collector's port but leave it dead (backend offline).
    let mock_port = reserve_port();
    // Vector's opentelemetry source requires both a grpc and an http listener; we only feed the http one,
    // but must reserve a free port for each.
    let http_port = reserve_port();
    let grpc_port = reserve_port();

    // The tempdir is Vector's buffer data_dir; it exists, so Vector just nests its buffer directory inside it.
    let tmp = tempfile::tempdir().unwrap();
    let config_path = edge_toolkit::config::get_project_root().join("config/vector-otlp-relay.yaml");

    // 2. Start Vector from the static config. Its sink target (mock_port) is down, so anything it accepts
    //    must be buffered to disk and retried.
    let mut child = Command::new("vector")
        .arg("-c")
        .arg(&config_path)
        .env("VECTOR_LOG", "warn")
        // Forward-slash the temp path: Vector interpolates it into a double-quoted YAML scalar, and on
        // Windows the backslashes would be parsed as YAML escapes (CI failed with "did not find expected
        // hexadecimal number"). Forward slashes are accepted on Windows too.
        .env("RELAY_DATA_DIR", tmp.path().to_string_lossy().replace('\\', "/"))
        .env("RELAY_GRPC_PORT", grpc_port.to_string())
        .env("RELAY_HTTP_PORT", http_port.to_string())
        .env("RELAY_SINK_PORT", mock_port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn_checked()
        .unwrap()
        .into_child();
    // Drain Vector's stderr into memory so it stays quiet on success but is available for failure messages
    // (a file-backed `Stdio` would need the banned `std::fs::File`).
    let log = drain_stderr(&mut child);
    // Kill Vector on every exit path (including panics) so no process leaks.
    let mut vector = ChildGuard::new(child);

    assert!(
        wait_for_port(http_port),
        "vector http source never came up on :{http_port}\n{}",
        stop_and_read(&mut vector, &log),
    );

    // 3. Emit one span through the real OTLP/HTTP exporter into Vector's source. Vector returns 200 and
    //    buffers the event because the sink can't reach the (dead) collector.
    et_test_otlp::emit_span(
        &format!("http://127.0.0.1:{http_port}/v1/traces"),
        HashMap::new(),
        SERVICE_NAME,
        SPAN_NAME,
    );

    // 4. Bring the collector online on the reserved port.
    //    This test is a plain sync `#[test]`, but `mock-collector` is async and spawns its server onto the
    //    *caller's* runtime, so the test has to own one. A multi-thread `Runtime` keeps polling the spawned
    //    server on its own worker threads once `block_on` returns -- which is what makes the later blocking
    //    steps (`Command`, `ChildGuard`, `thread::sleep`) safe. Vector's sink encodes `codec: otlp`
    //    (protobuf), and `mock-collector` fixes the encoding at construction rather than sniffing
    //    `Content-Type`, so the server must be built as `HttpBinary`.
    let runtime = Runtime::new().unwrap();
    let mock = runtime.block_on(et_test_otlp::start_on(mock_port, Protocol::HttpBinary));

    // 5. The buffered span must now be forwarded, intact.
    let Some(relayed) = wait_for_relayed_span(&runtime, &mock) else {
        panic!(
            "mock never received the relayed `{SPAN_NAME}` span -- store-and-forward failed\n{}",
            stop_and_read(&mut vector, &log),
        );
    };
    assert_eq!(relayed.name, SPAN_NAME, "relayed span name mismatch");
    assert_eq!(relayed.service_name, SERVICE_NAME, "relayed service.name mismatch");
    // The SDK generates the ids, so we can't assert exact values -- but a relayed span must carry both.
    assert!(!relayed.trace_id.is_empty(), "relayed span is missing its trace id");
    assert!(!relayed.span_id.is_empty(), "relayed span is missing its span id");
}

/// Wait until a span named [`SPAN_NAME`] arrives, flattened for the assertions above.
fn wait_for_relayed_span(runtime: &Runtime, mock: &ServerHandle) -> Option<et_test_otlp::FlatSpan> {
    runtime.block_on(et_test_otlp::wait_for_span(mock, SPAN_NAME, RELAY_TIMEOUT))
}

/// Shut Vector down so its stderr drainer reaches EOF, then return the captured log.
fn stop_and_read(vector: &mut ChildGuard, log: &Mutex<String>) -> String {
    vector.shutdown();
    // Give the drainer thread a moment to flush the final bytes.
    std::thread::sleep(Duration::from_millis(200));
    format!("--- vector stderr ---\n{}", log.lock().unwrap())
}
