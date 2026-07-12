//! Shared `OpenTelemetry` / OTLP setup for edge-toolkit services.
//!
//! Wires up:
//! - The W3C tracecontext propagator (so `traceparent` headers cross
//!   process boundaries on HTTP).
//! - An OTLP/HTTP span exporter (binary or JSON, per `OtlpConfig`).
//! - An OTLP/HTTP log exporter, exposed through `tracing` so `info!` and
//!   friends are forwarded.
//! - A `tracing` subscriber that fans `info!`/`error!` out to stdout *and*
//!   the `OTel` pipeline.
//!
//! Returns an `OtelHandles` which the caller must `shutdown()` before exit
//! so batched spans/logs are flushed -- otherwise short-lived processes
//! (e.g. the wasi-runner, which exits as soon as a module finishes) drop
//! their tail-end spans.
use edge_toolkit::config::{OtlpConfig, OtlpProtocol};
use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator};
use tracing::subscriber::set_global_default;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt as _};

pub const RUST_LOG: &str = "RUST_LOG";

/// Handles for the spans + logs + metrics pipelines.
///
/// Drop alone won't flush -- call [`OtelHandles::shutdown`] at the end of
/// `main()` (or in a Drop guard).
#[non_exhaustive]
pub struct OtelHandles {
    pub tracer_provider: SdkTracerProvider,
    pub logger_provider: SdkLoggerProvider,
    pub meter_provider: SdkMeterProvider,
}

impl OtelHandles {
    /// Flush any buffered spans/logs/metrics and tear down the exporters.
    pub fn shutdown(self) {
        // Errors here are non-fatal -- the process is exiting anyway.
        drop(self.tracer_provider.shutdown());
        drop(self.logger_provider.shutdown());
        drop(self.meter_provider.shutdown());
    }
}

/// Initialise the global tracing subscriber + `OTel` pipeline against `config`.
///
/// Call exactly once per process; a second call returns an error from
/// `set_global_default`. Exporter-build and `RUST_LOG`-parse failures are
/// returned too, so `main` can surface them and exit non-zero.
///
/// # Errors
///
/// Returns an error if any OTLP exporter fails to build, `RUST_LOG` is
/// invalid, or the global subscriber is already set.
pub fn init(config: &OtlpConfig) -> Result<OtelHandles, Box<dyn std::error::Error + Send + Sync>> {
    // tracing_log forwards `log` crate records (used by transitive deps)
    // through the tracing subscriber.
    drop(tracing_log::LogTracer::init());

    let mut headers = std::collections::HashMap::new();
    if let Some(auth) = &config.auth {
        auth.add_basic_auth_header(&mut headers);
    }

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let trace_endpoint = format!("{}/traces", config.collector_url);
    let log_endpoint = format!("{}/logs", config.collector_url);
    let protocol = match config.protocol {
        OtlpProtocol::Binary => opentelemetry_otlp::Protocol::HttpBinary,
        OtlpProtocol::JSON => opentelemetry_otlp::Protocol::HttpJson,
    };

    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(trace_endpoint)
        .with_headers(headers.clone())
        .build()?;

    let mut service_descriptors = vec![KeyValue::new("service.version", env!("CARGO_PKG_VERSION").to_string())];
    if let Some(hostname) = hostname::get().ok().and_then(|host| host.into_string().ok()) {
        service_descriptors.push(KeyValue::new("service.instance", hostname));
    }
    let resource = Resource::builder()
        .with_service_name(config.service_label.clone())
        .with_attributes(service_descriptors)
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    // Set the global tracer provider so direct `global::tracer(...)` spans (e.g. the ws hub's `ws.connect`)
    // export too -- not just the `tracing`-subscriber spans routed through the layer below.
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());

    let otel_tracing_layer = OpenTelemetryLayer::new(tracer_provider.tracer(config.service_label.clone()));

    // Metrics ride the same OTLP/HTTP transport as spans and logs, posting to `<collector_url>/metrics`.
    // The periodic reader batches on its own interval; `OtelHandles::shutdown` forces a final flush on exit.
    let metric_endpoint = format!("{}/metrics", config.collector_url);
    let metric_exporter = MetricExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(metric_endpoint)
        .with_headers(headers.clone())
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_resource(resource.clone())
        .build();
    opentelemetry::global::set_meter_provider(meter_provider.clone());

    let log_directives = std::env::var(RUST_LOG).unwrap_or_else(|_| "info".to_string());
    let env_filter = EnvFilter::try_new(log_directives)?;

    let log_exporter = LogExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(log_endpoint)
        .with_headers(headers)
        .build()?;

    let logger_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();

    let otel_log_layer = OpenTelemetryTracingBridge::new(&logger_provider);
    let stdout_fmt_layer = tracing_subscriber::fmt::layer().event_format(tracing_subscriber::fmt::format().compact());

    let subscriber = Registry::default()
        .with(env_filter)
        .with(stdout_fmt_layer)
        .with(otel_tracing_layer)
        .with(otel_log_layer);

    set_global_default(subscriber)?;

    Ok(OtelHandles {
        tracer_provider,
        logger_provider,
        meter_provider,
    })
}
