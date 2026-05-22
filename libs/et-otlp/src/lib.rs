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
//! so batched spans/logs are flushed — otherwise short-lived processes
//! (e.g. the wasi-runner, which exits as soon as a module finishes) drop
//! their tail-end spans.
#![expect(
    clippy::expect_used,
    reason = "init runs once at startup; exporter build / RUST_LOG / subscriber failures should crash early"
)]

use edge_toolkit::config::{OtlpConfig, OtlpProtocol};
use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator};
use tracing::subscriber::set_global_default;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt as _};

pub const RUST_LOG: &str = "RUST_LOG";

/// Handles for the spans + logs pipelines.
///
/// Drop alone won't flush — call [`OtelHandles::shutdown`] at the end of
/// `main()` (or in a Drop guard).
#[non_exhaustive]
pub struct OtelHandles {
    pub tracer_provider: SdkTracerProvider,
    pub logger_provider: SdkLoggerProvider,
}

impl OtelHandles {
    /// Flush any buffered spans/logs and tear down the exporters.
    pub fn shutdown(self) {
        // Errors here are non-fatal — the process is exiting anyway.
        drop(self.tracer_provider.shutdown());
        drop(self.logger_provider.shutdown());
    }
}

/// Initialise the global tracing subscriber + `OTel` pipeline against `config`.
///
/// Call exactly once per process; subsequent calls panic via
/// `set_global_default`.
#[must_use]
pub fn init(config: &OtlpConfig) -> OtelHandles {
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
        .build()
        .expect("build OTLP span exporter");

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

    let otel_tracing_layer = OpenTelemetryLayer::new(tracer_provider.tracer(config.service_label.clone()));

    let log_directives = std::env::var(RUST_LOG).unwrap_or_else(|_| "info".to_string());
    let env_filter = EnvFilter::try_new(log_directives).expect("valid RUST_LOG");

    let log_exporter = LogExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(log_endpoint)
        .with_headers(headers)
        .build()
        .expect("build OTLP log exporter");

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

    set_global_default(subscriber).expect("set tracing subscriber");

    OtelHandles {
        tracer_provider,
        logger_provider,
    }
}
