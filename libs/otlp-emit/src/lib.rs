//! In-process OTLP/HTTP span emitter built on the real `OpenTelemetry` SDK, for integration tests.
//!
//! The counterpart to `int-otlp-mock`: where the mock *receives* OTLP, this *sends* it, through the same
//! `opentelemetry-otlp` HTTP exporter the services use via `et-otlp`. Tests therefore exercise the real emit
//! path -- SDK span -> OTLP/protobuf over HTTP -- instead of hand-building a protobuf request. Point it at any
//! OTLP/HTTP traces endpoint: a mock collector, a Vector `opentelemetry` source, or a live o2 server.
#![expect(
    clippy::unwrap_used,
    reason = "test-support emitter: a failed exporter build must fail the test loudly"
)]

use std::collections::HashMap;

use opentelemetry::KeyValue;
use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;

/// Emit one span named `span_name` under `service_name` to the OTLP/HTTP traces `endpoint`, then flush.
///
/// `endpoint` is the exact URL to POST to (e.g. `http://host:4318/v1/traces`, or o2's
/// `.../api/{org}/v1/traces`); `headers` carries any auth the collector needs (o2 wants HTTP basic auth),
/// and is empty for an unauthenticated one. Each emitted span also carries a `probe` attribute set to
/// `span_name`, so receivers that expose attributes can match on it. A blocking exporter behind a simple
/// (synchronous) span processor is used, so the span has been exported by the time this returns -- no async
/// runtime and no batch-flush race. Panics on exporter-build failure: in a test that means a misconfigured
/// environment, which must fail loudly rather than silently skip.
#[expect(
    clippy::implicit_hasher,
    reason = "test-support: callers pass a std-hasher header map"
)]
pub fn emit_span(endpoint: &str, headers: HashMap<String, String>, service_name: &str, span_name: &str) {
    let exporter = SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .with_headers(headers)
        .build()
        .unwrap();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .with_resource(Resource::builder().with_service_name(service_name.to_owned()).build())
        .build();
    let tracer = provider.tracer("int-otlp-emit");
    let mut span = tracer
        .span_builder(span_name.to_owned())
        .with_attributes([KeyValue::new("probe", span_name.to_owned())])
        .start(&tracer);
    span.end();
    // Simple processor exports on span end; shutdown then flushes and tears the exporter down. Discard the
    // result -- the process is a test and there is no caller to propagate an exporter teardown error to.
    let _shutdown = provider.shutdown();
}
