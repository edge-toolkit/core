//! Offline store-and-forward relay proof, using the real Vector binary.
//!
//! The runners must keep working while offline: telemetry has to be buffered
//! locally and forwarded once the backend reappears. This test proves an
//! off-the-shelf OSS relay (Vector's `opentelemetry` source -> disk buffer ->
//! `opentelemetry` sink) does exactly that, end to end:
//!
//!   1. Reserve a free port for the collector, but do NOT start it -- the
//!      backend is "offline".
//!   2. Start `vector` with its sink pointed at that dead port and a disk
//!      buffer, so accepted events are stored, not dropped.
//!   3. POST one canonical OTLP/protobuf trace into Vector's source. Vector
//!      accepts it (200) and buffers it, since the sink target is down.
//!   4. Bring the mock collector up on the reserved port ("back online").
//!   5. Assert the buffered span is forwarded to the mock, intact -- same
//!      name, service, trace id and span id we sent.
//!
//! The Vector config is the static, checked-in `config/vector-otlp-relay.yaml`;
//! per-run ports and the buffer's `data_dir` are passed as `RELAY_*` env vars,
//! which Vector interpolates. Vector's JSON OTLP encoding does not emit a valid
//! `{resourceSpans:...}` envelope (vectordotdev/vector#23971), so the reliable
//! wire format both into and out of Vector is protobuf on the `/traces` path.

#![cfg(test)]
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::single_call_fn,
    reason = "test code: loud failures and named single-use step helpers"
)]

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use et_test_helpers::{ChildGuard, drain_stderr, reserve_port, wait_for_port};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message as _;
use retry::delay::Fixed;
use retry::retry;

const SERVICE_NAME: &str = "vector-relay-test";
const SPAN_NAME: &str = "relay-probe";
const TRACE_ID: [u8; 16] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01,
];
const SPAN_ID: [u8; 8] = [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18];

#[test]
fn vector_relays_buffered_otlp_after_backend_comes_online() {
    // 1. Reserve the collector's port but leave it dead (backend offline).
    let mock_port = reserve_port();
    // Vector's opentelemetry source requires both a grpc and an http listener;
    // we only feed the http one, but must reserve a free port for each.
    let http_port = reserve_port();
    let grpc_port = reserve_port();

    // The tempdir is Vector's buffer data_dir; it exists, so Vector just nests
    // its buffer directory inside it.
    let tmp = tempfile::tempdir().expect("create tempdir");
    let config_path = edge_toolkit::config::get_project_root().join("config/vector-otlp-relay.yaml");

    // 2. Start Vector from the static config. Its sink target (mock_port) is
    //    down, so anything it accepts must be buffered to disk and retried.
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
        .spawn()
        .expect("spawn vector");
    // Drain Vector's stderr into memory so it stays quiet on success but is
    // available for failure messages (a file-backed `Stdio` would need the
    // banned `std::fs::File`).
    let log = drain_stderr(&mut child);
    // Kill Vector on every exit path (including panics) so no process leaks.
    let mut vector = ChildGuard::new(child);

    assert!(
        wait_for_port(http_port),
        "vector http source never came up on :{http_port}\n{}",
        stop_and_read(&mut vector, &log),
    );

    // 3. Push one OTLP/protobuf trace into Vector's source. It returns 200 and
    //    buffers the event because the sink can't reach the (dead) collector.
    let response = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/v1/traces"))
        .header("content-type", "application/x-protobuf")
        .body(otlp_trace_request())
        .send()
        .expect("POST OTLP to vector source");
    assert!(
        response.status().is_success(),
        "vector source rejected the OTLP push: {}",
        response.status()
    );

    // 4. Bring the collector online on the reserved port.
    let mock = int_otlp_mock::start_on(mock_port);

    // 5. The buffered span must now be forwarded, intact.
    let Some(relayed) = wait_for_relayed_span(&mock) else {
        panic!(
            "mock never received the relayed `{SPAN_NAME}` span -- store-and-forward failed\n{}",
            stop_and_read(&mut vector, &log),
        );
    };

    assert_eq!(relayed.name, SPAN_NAME, "relayed span name mismatch");
    assert_eq!(relayed.service_name, SERVICE_NAME, "relayed service.name mismatch");
    assert_eq!(
        relayed.trace_id,
        int_otlp_mock::to_hex(&TRACE_ID),
        "relayed trace id mismatch"
    );
    assert_eq!(
        relayed.span_id,
        int_otlp_mock::to_hex(&SPAN_ID),
        "relayed span id mismatch"
    );
}

/// Build a canonical single-span `ExportTraceServiceRequest`, protobuf-encoded.
fn otlp_trace_request() -> Vec<u8> {
    let span = Span {
        trace_id: TRACE_ID.to_vec(),
        span_id: SPAN_ID.to_vec(),
        name: SPAN_NAME.to_owned(),
        start_time_unix_nano: 1,
        end_time_unix_nano: 2,
        ..Default::default()
    };
    let resource = Resource {
        attributes: vec![KeyValue {
            key: "service.name".to_owned(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(SERVICE_NAME.to_owned())),
            }),
        }],
        ..Default::default()
    };
    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource),
            scope_spans: vec![ScopeSpans {
                spans: vec![span],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    request.encode_to_vec()
}

/// Poll the mock (via `retry`) until a span named [`SPAN_NAME`] arrives; ~30s.
fn wait_for_relayed_span(mock: &int_otlp_mock::OtlpMock) -> Option<int_otlp_mock::FlatSpan> {
    retry(Fixed::from_millis(250).take(120), || {
        mock.flatten_spans()
            .into_iter()
            .find(|span| span.name == SPAN_NAME)
            .ok_or(())
    })
    .ok()
}

/// Shut Vector down so its stderr drainer reaches EOF, then return the captured log.
fn stop_and_read(vector: &mut ChildGuard, log: &Mutex<String>) -> String {
    vector.shutdown();
    // Give the drainer thread a moment to flush the final bytes.
    std::thread::sleep(Duration::from_millis(200));
    format!("--- vector stderr ---\n{}", log.lock().expect("log mutex"))
}
