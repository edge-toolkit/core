//! The hub's spans and metrics, recorded through the global tracer and meter `et_otlp::init` installs.
//!
//! Every one of them is a no-op without the `otel` feature, so the hub's code records them unconditionally.

#[cfg(feature = "otel")]
use std::sync::LazyLock;

#[cfg(feature = "otel")]
use opentelemetry::global::{self, BoxedSpan};
#[cfg(feature = "otel")]
use opentelemetry::metrics::{Counter, UpDownCounter};
#[cfg(feature = "otel")]
use opentelemetry::trace::{Span as _, Tracer as _};

// Built lazily on first use -- by then the meter provider is set -- and cached for the process.
#[cfg(feature = "otel")]
static MESSAGES_RECEIVED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("ws-server")
        .u64_counter("et_ws.messages.received")
        .with_description("Inbound WebSocket frames the hub has handled")
        .build()
});
#[cfg(feature = "otel")]
static ACTIVE_CONNECTIONS: LazyLock<UpDownCounter<i64>> = LazyLock::new(|| {
    global::meter("ws-server")
        .i64_up_down_counter("et_ws.connections.active")
        .with_description("Currently-open WebSocket connections")
        .build()
});

/// Count one inbound WebSocket frame.
#[expect(
    clippy::single_call_fn,
    reason = "keeps the meter, and its absence without `otel`, out of the frame loop"
)]
#[cfg_attr(
    not(feature = "otel"),
    expect(clippy::missing_const_for_fn, reason = "it is empty only without `otel`")
)]
pub(crate) fn message_received() {
    #[cfg(feature = "otel")]
    MESSAGES_RECEIVED.add(1, &[]);
}

/// Count `delta` WebSocket connections opening, or closing when negative.
#[cfg_attr(
    not(feature = "otel"),
    expect(unused_variables, reason = "there is no meter to count on without `otel`")
)]
#[cfg_attr(
    not(feature = "otel"),
    expect(clippy::missing_const_for_fn, reason = "it is empty only without `otel`")
)]
pub(crate) fn connections_changed(delta: i64) {
    #[cfg(feature = "otel")]
    ACTIVE_CONNECTIONS.add(delta, &[]);
}

/// A span of the hub's, ended when it is dropped.
pub(crate) struct HubSpan {
    #[cfg(feature = "otel")]
    span: BoxedSpan,
}

impl HubSpan {
    /// Start the span `name`.
    #[cfg_attr(
        not(feature = "otel"),
        expect(unused_variables, reason = "there is no tracer to start a span on without `otel`")
    )]
    #[cfg_attr(
        not(feature = "otel"),
        expect(clippy::missing_const_for_fn, reason = "it holds no span to start without `otel`")
    )]
    pub(crate) fn start(name: &'static str) -> Self {
        Self {
            #[cfg(feature = "otel")]
            span: global::tracer("ws-server").start(name),
        }
    }
}

#[cfg(feature = "otel")]
impl Drop for HubSpan {
    fn drop(&mut self) {
        self.span.end();
    }
}
