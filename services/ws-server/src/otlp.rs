use edge_toolkit::config::{OtlpConfig, OtlpProtocol};
use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator};
use tracing::subscriber::set_global_default;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt};

pub const RUST_LOG: &str = "RUST_LOG";

// Initialize OpenTelemetry.
pub fn init(config: &OtlpConfig) -> SdkTracerProvider {
    tracing_log::LogTracer::init().unwrap();

    let mut telemetry_collector_headers = std::collections::HashMap::new();
    if let Some(auth) = &config.auth {
        auth.add_basic_auth_header(&mut telemetry_collector_headers);
    }

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let trace_endpoint = format!("{}/traces", config.collector_url.clone());
    let log_endpoint = format!("{}/logs", config.collector_url.clone());

    let protocol = match config.protocol {
        OtlpProtocol::Binary => opentelemetry_otlp::Protocol::HttpBinary,
        OtlpProtocol::JSON => opentelemetry_otlp::Protocol::HttpJson,
    };

    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(trace_endpoint)
        .with_headers(telemetry_collector_headers.clone())
        .build()
        .unwrap();

    let mut service_descriptors = vec![KeyValue::new("service.version", env!("CARGO_PKG_VERSION").to_string())];
    if let Some(hostname) = hostname::get().ok().and_then(|h| h.into_string().ok()) {
        service_descriptors.push(KeyValue::new("service.instance", hostname));
    }

    let resource = Resource::builder()
        .with_service_name(config.service_label.clone())
        .with_attributes(service_descriptors)
        .build();

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource.clone())
        .build();

    let otel_tracing_layer = OpenTelemetryLayer::new(provider.tracer(config.service_label.clone()));

    let log_directives = if let Ok(level) = std::env::var(RUST_LOG) {
        log::info!("{RUST_LOG}={level}");
        level
    } else {
        log::info!("{RUST_LOG} defaulted to info");
        "info".to_string()
    };
    let env_filter = EnvFilter::try_new(log_directives).unwrap();

    let exporter = LogExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(log_endpoint)
        .with_headers(telemetry_collector_headers)
        .build()
        .unwrap();

    let log_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let otel_layer = OpenTelemetryTracingBridge::new(&log_provider);

    let stdout_format = tracing_subscriber::fmt::format().compact();

    let stdout_fmt_layer = tracing_subscriber::fmt::layer().event_format(stdout_format);

    let subscriber = Registry::default()
        .with(env_filter)
        .with(stdout_fmt_layer)
        .with(otel_tracing_layer)
        .with(otel_layer);

    set_global_default(subscriber).unwrap();
    provider
}
