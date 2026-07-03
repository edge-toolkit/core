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
//! Vector's JSON OTLP encoding does not emit a valid `{resourceSpans:...}`
//! envelope (vectordotdev/vector#23971), so the reliable wire format both into
//! and out of Vector is protobuf on the standard `/v1/traces` path -- which is
//! why this drives the mock's protobuf endpoint rather than its JSON one.

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::single_call_fn,
    reason = "test code: deadline arithmetic, loud failures, a skip notice, and named single-use step helpers"
)]

use std::io::Read as _;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fs_err as fs;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message as _;

const SERVICE_NAME: &str = "vector-relay-test";
const SPAN_NAME: &str = "relay-probe";
const TRACE_ID: [u8; 16] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01,
];
const SPAN_ID: [u8; 8] = [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18];

// Not validated on the Windows runner yet: this spawns the `vector` binary and
// binds loopback ports; the process/path handling wants a Windows check first.
#[test]
#[cfg_attr(windows, ignore = "vector subprocess relay not yet validated on Windows")]
fn vector_relays_buffered_otlp_after_backend_comes_online() {
    if !vector_available() {
        // `vector` is a mise [tools] entry; a bare `cargo test` outside the
        // mise shim PATH won't see it. Skip rather than fail there.
        eprintln!("skipping: `vector` binary not on PATH (run under `mise exec -- cargo test`)");
        return;
    }

    // 1. Reserve the collector's port but leave it dead (backend offline).
    let mock_port = reserve_port();
    // Vector's opentelemetry source requires both a grpc and an http listener;
    // we only feed the http one, but must bind a free port for each.
    let http_port = reserve_port();
    let grpc_port = reserve_port();

    let tmp = tempfile::tempdir().expect("create tempdir");
    let data_dir = tmp.path().join("data");
    fs::create_dir_all(&data_dir).expect("create vector data_dir");
    let config_path = tmp.path().join("vector.yaml");
    fs::write(&config_path, vector_config(&data_dir, http_port, grpc_port, mock_port)).expect("write vector config");

    // 2. Start Vector. Its sink target (mock_port) is down, so anything it
    //    accepts must be buffered to the disk buffer and retried, not dropped.
    let mut child = Command::new("vector")
        .arg("-c")
        .arg(&config_path)
        .env("VECTOR_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn vector");
    // Drain Vector's stderr into memory so it stays quiet on success but is
    // available for failure messages (a file-backed `Stdio` would need the
    // banned `std::fs::File`).
    let log = drain_stderr(&mut child);
    // Kill Vector on every exit path (including panics) so no process leaks.
    let mut vector = ChildGuard(child);

    assert!(
        wait_for_port(http_port, Duration::from_secs(20)),
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
    let Some(relayed) = wait_for_relayed_span(&mock, Duration::from_secs(30)) else {
        panic!(
            "mock never received the relayed `{SPAN_NAME}` span within 30s -- store-and-forward failed\n{}",
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

/// Render the Vector config: OTLP http/grpc source -> disk buffer -> OTLP sink.
fn vector_config(data_dir: &Path, http_port: u16, grpc_port: u16, mock_port: u16) -> String {
    // The sink's disk buffer + capped retry backoff mean events are stored
    // while the collector is down and re-attempted every few seconds until it
    // appears. `healthcheck.enabled: false` lets Vector start with the sink
    // target still offline. `use_otlp_decoding.traces` + `encoding.codec: otlp`
    // preserve the OTLP payload through Vector unchanged (protobuf in, out).
    format!(
        "data_dir: {data_dir}
sources:
  in:
    type: opentelemetry
    grpc:
      address: 127.0.0.1:{grpc_port}
    http:
      address: 127.0.0.1:{http_port}
    use_otlp_decoding:
      traces: true
sinks:
  out:
    type: opentelemetry
    inputs:
      - in.traces
    healthcheck:
      enabled: false
    buffer:
      type: disk
      max_size: 268435488
      when_full: block
    protocol:
      type: http
      uri: http://127.0.0.1:{mock_port}/traces
      method: post
      encoding:
        codec: otlp
      request:
        retry_initial_backoff_secs: 1
        retry_max_duration_secs: 5
",
        data_dir = data_dir.display(),
    )
}

/// Poll the mock until a span named [`SPAN_NAME`] arrives, or the timeout hits.
fn wait_for_relayed_span(mock: &int_otlp_mock::OtlpMock, timeout: Duration) -> Option<int_otlp_mock::FlatSpan> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(span) = mock.flatten_spans().into_iter().find(|span| span.name == SPAN_NAME) {
            return Some(span);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// True if a `vector` binary is resolvable and runnable on PATH.
fn vector_available() -> bool {
    Command::new("vector")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Reserve a free loopback TCP port by binding `:0` and releasing it.
fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("read local addr")
        .port()
}

/// Wait until `port` accepts a TCP connection, up to `timeout`.
fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Spawn a thread that drains the child's stderr into a shared buffer.
fn drain_stderr(child: &mut Child) -> Arc<Mutex<String>> {
    let stderr = child.stderr.take().expect("capture vector stderr");
    let log = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&log);
    drop(std::thread::spawn(move || {
        let mut buffer = String::new();
        // Reads until Vector exits (EOF); failure to read leaves the buffer empty.
        drop(std::io::BufReader::new(stderr).read_to_string(&mut buffer));
        *sink.lock().expect("log mutex") = buffer;
    }));
    log
}

/// Kill Vector so its stderr drainer reaches EOF, then return the captured log.
fn stop_and_read(vector: &mut ChildGuard, log: &Mutex<String>) -> String {
    drop(vector.0.kill());
    drop(vector.0.wait());
    // Give the drainer thread a moment to flush the final bytes.
    std::thread::sleep(Duration::from_millis(200));
    format!("--- vector stderr ---\n{}", log.lock().expect("log mutex"))
}

/// Kills the child Vector process when dropped, so panics don't leak it.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
}
