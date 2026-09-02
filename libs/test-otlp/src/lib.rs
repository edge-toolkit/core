//! OTLP test support: both ends of the wire.
//!
//! [`emit_span`] *sends* OTLP through the real `opentelemetry-otlp` HTTP exporter the services use via
//! `et-otlp`, so tests exercise the genuine emit path rather than hand-building a protobuf request. The rest
//! of the crate helps assert on what was *received*: the collector itself is the third-party `mock-collector`
//! crate, and this supplies the pieces the workspace's integration tests kept re-deriving -- the
//! `/v1`-suffixed collector URL `et_otlp::init` expects, and flat span/metric views pairing each record with
//! its resource's `service.name`, so a test can group by service without walking the nested
//! `Resource -> Scope -> record` protobuf structure itself.
//!
//! `mock-collector` fixes a server's wire encoding at construction rather than sniffing `Content-Type`, so a
//! test covering both encodings starts two servers -- see this crate's own `logs` and `metrics` tests.
#![expect(
    clippy::unwrap_used,
    reason = "test support; an unbindable collector or unbuildable exporter means an unusable environment"
)]

use std::collections::HashMap;
use std::time::Duration;

use mock_collector::{MockCollector, MockServerBuilder};
/// Re-exported so call sites need no direct `mock-collector` dependency.
///
/// `Protocol` names the wire encoding a server speaks; `ServerHandle` is the running collector itself.
pub use mock_collector::{Protocol, ServerHandle};
use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
use opentelemetry_otlp::{Protocol as ExportProtocol, SpanExporter, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{metric::Data, number_data_point};
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
        .with_protocol(ExportProtocol::HttpBinary)
        .with_endpoint(endpoint)
        .with_headers(headers)
        .build()
        .unwrap();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .with_resource(Resource::builder().with_service_name(service_name.to_owned()).build())
        .build();
    let tracer = provider.tracer("et-test-otlp");
    let mut span = tracer
        .span_builder(span_name.to_owned())
        .with_attributes([opentelemetry::KeyValue::new("probe", span_name.to_owned())])
        .start(&tracer);
    span.end();
    // Simple processor exports on span end; shutdown then flushes and tears the exporter down. Discard the
    // result -- the process is a test and there is no caller to propagate an exporter teardown error to.
    let _shutdown = provider.shutdown();
}

/// Start a collector on an OS-assigned port, speaking `protocol`.
///
/// # Panics
///
/// Panics if the server cannot bind, which in a test means the environment is unusable.
pub async fn start(protocol: Protocol) -> ServerHandle {
    MockServerBuilder::new().protocol(protocol).start().await.unwrap()
}

/// Start a collector on a caller-chosen port (which must be free), speaking `protocol`.
///
/// Store-and-forward tests reserve a free port, point a relay's sink at it while nothing is listening, then
/// call this to bring the collector up on the same port -- proving the relay's buffered data gets delivered
/// once the backend appears.
///
/// # Panics
///
/// Panics if the server cannot bind the requested port.
pub async fn start_on(port: u16, protocol: Protocol) -> ServerHandle {
    MockServerBuilder::new()
        .protocol(protocol)
        .port(port)
        .start()
        .await
        .unwrap()
}

/// The value to hand `OTLP_COLLECTOR_URL` so exporters target `handle`.
///
/// `mock-collector` serves OTLP's spec paths (`/v1/traces`, ...) while `et_otlp::init` builds its endpoints as
/// `{collector_url}/traces`, so the `/v1` belongs in the base URL.
#[must_use]
pub fn collector_url(handle: &ServerHandle) -> String {
    format!("http://{}/v1", handle.addr())
}

/// Snapshot every captured span, flattened and paired with its service name.
pub async fn flatten_spans(handle: &ServerHandle) -> Vec<FlatSpan> {
    handle.with_collector(flatten_spans_in).await
}

/// Snapshot every captured metric, flattened and paired with its service name.
pub async fn flatten_metrics(handle: &ServerHandle) -> Vec<FlatMetric> {
    handle.with_collector(flatten_metrics_in).await
}

/// Wait for a span named `name` to arrive, then return the flattened view of it.
///
/// Returns `None` if `timeout` passes without one arriving.
pub async fn wait_for_span(handle: &ServerHandle, name: &str, timeout: Duration) -> Option<FlatSpan> {
    handle
        .wait_until(
            |collector| collector.spans().iter().any(|span| span.span().name == name),
            timeout,
        )
        .await
        .ok()?;
    flatten_spans(handle).await.into_iter().find(|span| span.name == name)
}

/// Walk every span in `collector`, pairing each with its `Resource`'s `service.name`.
///
/// Trace/span ids are lowercase-hex-encoded from the decoded bytes -- compare against `const_hex::encode` of
/// the raw ids you sent.
#[expect(
    clippy::single_call_fn,
    reason = "named fn-pointer argument to with_collector; the closure form would inline the whole walk"
)]
fn flatten_spans_in(collector: &MockCollector) -> Vec<FlatSpan> {
    collector
        .spans()
        .iter()
        .map(|test_span| FlatSpan {
            service_name: service_name(test_span.resource_attrs()),
            trace_id: const_hex::encode(&test_span.span().trace_id),
            span_id: const_hex::encode(&test_span.span().span_id),
            parent_span_id: const_hex::encode(&test_span.span().parent_span_id),
            name: test_span.span().name.clone(),
        })
        .collect()
}

/// Walk every metric in `collector`, pairing each with its `Resource`'s `service.name`.
///
/// `value` sums the numeric (Sum/Gauge) data points -- so a monotonic counter reads as its running total --
/// while `data_points` counts them (histogram points are counted but don't contribute to `value`).
#[expect(
    clippy::single_call_fn,
    reason = "named fn-pointer argument to with_collector; the closure form would inline the whole walk"
)]
fn flatten_metrics_in(collector: &MockCollector) -> Vec<FlatMetric> {
    collector
        .metrics()
        .iter()
        .map(|test_metric| {
            let metric = test_metric.metric();
            let (value, data_points) = match &metric.data {
                Some(Data::Sum(sum)) => sum_number_points(&sum.data_points),
                Some(Data::Gauge(gauge)) => sum_number_points(&gauge.data_points),
                Some(Data::Histogram(histogram)) => (0, histogram.data_points.len()),
                _ => (0, 0),
            };
            FlatMetric {
                service_name: service_name(test_metric.resource_attrs()),
                name: metric.name.clone(),
                unit: metric.unit.clone(),
                value,
                data_points,
            }
        })
        .collect()
}

/// Read the `service.name` string out of a record's resource attributes, or `""` when absent.
///
/// A non-string `service.name` reads as absent: the flattener has no meaningful `String` to report for it, and
/// tests group by the empty name rather than inventing a rendering of the other `AnyValue` shapes.
fn service_name(resource_attrs: &[KeyValue]) -> String {
    resource_attrs
        .iter()
        .filter(|attr| attr.key == "service.name")
        .find_map(|attr| {
            let any_value::Value::StringValue(value) = attr.value.as_ref()?.value.as_ref()? else {
                return None;
            };
            Some(value.clone())
        })
        .unwrap_or_default()
}

/// Sum the integer OTLP data points into `(total, count)`; float data points don't contribute to `total`.
fn sum_number_points(points: &[opentelemetry_proto::tonic::metrics::v1::NumberDataPoint]) -> (i64, usize) {
    let mut total: i64 = 0;
    for point in points {
        if let Some(number_data_point::Value::AsInt(value)) = point.value {
            total = total.saturating_add(value);
        }
    }
    (total, points.len())
}

/// Flattened span view for assertions.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FlatSpan {
    pub service_name: String,
    /// Lowercase-hex-encoded 16-byte trace id.
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
}

/// Flattened metric view for assertions.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FlatMetric {
    pub service_name: String,
    pub name: String,
    pub unit: String,
    /// Sum of the metric's integer (Sum/Gauge) data points -- a monotonic `u64`/`i64` counter's running total.
    /// Floating-point data points are ignored (the metrics emitted here are integer counters).
    pub value: i64,
    /// Number of data points seen for this metric (includes histogram points).
    pub data_points: usize,
}
