//! Implements `wasi:logging/logging`. Levels are routed into the `tracing`
//! macros so log lines flow through whatever subscriber the runner installed
//! (stdout fmt layer in dev, `OTel` logs in production). `context` is attached
//! as a structured field rather than baked into the message.

use crate::HostState;
use crate::bindings::wasi::logging::logging::{Host, Level};

impl Host for HostState {
    #[expect(
        clippy::cognitive_complexity,
        clippy::unused_async_trait_impl,
        reason = "a match arm per WASI level is the readable shape; the generated Host trait declares log
        async, and the tracing macros never await"
    )]
    async fn log(&mut self, level: Level, context: String, message: String) {
        match level {
            Level::Trace => tracing::trace!(target: "wasi_logging", context = %context, "{message}"),
            Level::Debug => tracing::debug!(target: "wasi_logging", context = %context, "{message}"),
            Level::Info => tracing::info!(target: "wasi_logging", context = %context, "{message}"),
            Level::Warn => tracing::warn!(target: "wasi_logging", context = %context, "{message}"),
            Level::Error => tracing::error!(target: "wasi_logging", context = %context, "{message}"),
            // `tracing` has no `critical` level. Route to error and tag the
            // attribute so a log processor can distinguish if it cares.
            Level::Critical => {
                tracing::error!(target: "wasi_logging", context = %context, critical = true, "{message}");
            }
        }
    }
}
