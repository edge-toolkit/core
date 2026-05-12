//! End-to-end trace-context propagation test.
//!
//! Wires up:
//!   1. A mock OTLP collector (in-process, on a free port).
//!   2. A ws-test-server (in-process, with OTLP init pointing at the mock,
//!      so its `TracingLogger` middleware emits server spans here).
//!   3. The `et-ws-wasi-runner` binary spawned as a child process, with
//!      `OTLP_COLLECTOR_URL` pointing at the same mock + service name set
//!      to `et-ws-wasi-runner`.
//!
//! Then asserts that at least one trace id appears in spans from both the
//! `et-ws-server`/`et-ws-test` resource and the `et-ws-wasi-runner`
//! resource — i.e. the runner's outgoing `traceparent` was extracted by
//! the server's `TracingLogger`, so the two processes share a trace.
//!
//! The wasi-data1 module makes two HTTP calls (GET package.json, GET
//! .wasm) before exiting, so even though the test only runs one module,
//! we should see ≥2 server spans and ≥3 runner spans (the `run_module`
//! parent + two child fetch spans) on a successful run. It's used here
//! instead of wasi-graphics-info because it's the cheapest WASI module
//! to exercise — no wgpu / wasi-nn work.

use std::collections::HashSet;
use std::time::Duration;

use edge_toolkit::config::{OtlpConfig, OtlpProtocol};

#[test]
fn trace_ids_propagate_between_runner_and_server() {
    // 1. Start the mock collector. Both processes will export to it.
    let mock = otlp_mock::start();

    // 2. Init OTLP in the test process *before* spawning the test server,
    //    so the global tracing subscriber + propagator are in place when
    //    actix-web's TracingLogger fires.
    //
    //    The service.name lets us distinguish server-side spans from the
    //    runner subprocess's spans in the captured payloads.
    // OtlpConfig is `non_exhaustive`, so build via Default + field
    // assignment.
    let mut server_otlp = OtlpConfig::default();
    server_otlp.collector_url = mock.collector_url.clone();
    server_otlp.protocol = OtlpProtocol::JSON;
    server_otlp.service_label = "et-ws-test".to_string();
    server_otlp.auth = None;
    let server_handles = et_otlp::init(&server_otlp);

    let server = et_ws_test_server::start();

    // 3. Spawn the runner pointed at both the test server *and* the mock
    //    OTLP. Every `OTLP_*` env var is consumed by the runner's
    //    `serde_env::from_env::<EnvConfig>()` call.
    let bin = env!("CARGO_BIN_EXE_et-ws-wasi-runner");
    let status = std::process::Command::new(bin)
        .env("RUNNER_MODULE", "et-ws-wasi-data1")
        .env("WS_SERVER_URL", &server.ws_url)
        .env("OTLP_COLLECTOR_URL", &mock.collector_url)
        .env("OTLP_PROTOCOL", "JSON")
        .env("OTLP_SERVICE_LABEL", "et-ws-wasi-runner")
        .status()
        .expect("failed to spawn et-ws-wasi-runner");

    assert!(status.success(), "runner exited with code {:?}", status.code());

    // 4. Flush our own (server-side) batch exporter so any pending spans
    //    land in the mock before we read it.
    server_handles.shutdown();
    // BatchExporter's HTTP POST is async-on-its-own-runtime — give it a
    // moment to drain. The runner subprocess already shut its provider
    // down before exiting (see services/ws-wasi-runner/src/main.rs).
    std::thread::sleep(Duration::from_millis(500));

    // 5. Inspect captured spans.
    let spans = mock.flatten_spans();
    assert!(
        !spans.is_empty(),
        "mock OTLP received zero spans — exporters may not be flushing"
    );

    let trace_ids_by_service: std::collections::HashMap<String, HashSet<String>> =
        spans.iter().fold(std::collections::HashMap::new(), |mut acc, span| {
            acc.entry(span.service_name.clone())
                .or_default()
                .insert(span.trace_id.clone());
            acc
        });

    let server_trace_ids = trace_ids_by_service.get("et-ws-test").cloned().unwrap_or_default();
    let runner_trace_ids = trace_ids_by_service
        .get("et-ws-wasi-runner")
        .cloned()
        .unwrap_or_default();

    assert!(
        !server_trace_ids.is_empty(),
        "no spans from `et-ws-test` service. captured: {:#?}",
        trace_ids_by_service
    );
    assert!(
        !runner_trace_ids.is_empty(),
        "no spans from `et-ws-wasi-runner` service. captured: {:#?}",
        trace_ids_by_service
    );

    let shared: Vec<&String> = server_trace_ids.intersection(&runner_trace_ids).collect();
    assert!(
        !shared.is_empty(),
        concat!(
            "no trace id was emitted by *both* processes — propagation failed.\n",
            "server trace ids: {:?}\n",
            "runner trace ids: {:?}",
        ),
        server_trace_ids,
        runner_trace_ids,
    );

    // The server's request span should be a child of one of the runner's
    // spans (parentSpanId points back into the runner's trace), proving
    // the propagation direction (runner → server).
    let server_with_parent = spans
        .iter()
        .filter(|s| s.service_name == "et-ws-test" && !s.parent_span_id.is_empty())
        .count();
    assert!(
        server_with_parent > 0,
        "no server span had a non-empty parentSpanId: TracingLogger didnt extract `traceparent`. server spans: {:#?}",
        spans
            .iter()
            .filter(|s| s.service_name == "et-ws-test")
            .collect::<Vec<_>>()
    );
}
