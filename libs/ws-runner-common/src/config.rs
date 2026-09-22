//! Environment-derived configuration shared by both native runners.
//!
//! Each runner deserialises its own top-level `Config` from the process environment via `serde-env`, nesting these
//! structs under `runner` / `ws` fields. With serde-env's `_`-segmented mapping that puts every `RUNNER_*` var under
//! [`RunnerConfig`] and every `WS_*` var under [`WsConfig`], so the two runners parse the common variables identically.

use std::time::Duration;

use edge_toolkit::ports::Services;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

/// Declare a runner's `Config`, with the fields every runner reads supplied and its own added after.
///
/// A macro rather than a shared struct the runners nest or flatten. Nesting changes the variable names
/// serde-env derives, and `#[serde(flatten)]` cannot rebuild these: serde-env buffers a flattened value as a
/// string, so the inner struct fails with `invalid type: string "", expected struct RunnerConfig`. Expanding
/// the fields leaves each runner deserialising exactly as it did while declaring them once.
///
/// What that buys is the drift: all three runners read the same three groups -- which module, which hub,
/// where telemetry goes -- and while each spelled them out, two of them silently had no `otlp` at all.
///
/// The expansion names `::et_otlp` and `::edge_toolkit`, so a crate invoking this needs both as direct
/// dependencies even where its own code mentions neither. Dropping one as unused fails at the call site
/// with `cannot find et_otlp in the crate root`, pointing at the macro rather than at the manifest.
#[macro_export]
macro_rules! runner_config {
    (
        $(#[$struct_meta:meta])*
        $vis:vis struct $name:ident { $($(#[$field_meta:meta])* $field_vis:vis $field:ident : $ty:ty),* $(,)? }
    ) => {
        $(#[$struct_meta])*
        #[derive(Clone, Debug, ::serde::Deserialize)]
        #[non_exhaustive]
        $vis struct $name {
            /// `RUNNER_*` settings (`RUNNER_MODULE`, `RUNNER_TIMEOUT`).
            pub runner: $crate::config::RunnerConfig,
            /// `WS_*` settings (`WS_SERVER_URL`).
            #[serde(default)]
            pub ws: $crate::config::WsConfig,
            /// `OTLP_*` settings; `None` logs to stderr instead of exporting.
            #[serde(default)]
            pub otlp: ::core::option::Option<::edge_toolkit::config::OtlpConfig>,
            $($(#[$field_meta])* $field_vis $field : $ty,)*
        }

        impl ::et_otlp::Telemetered for $name {
            fn otlp(&self) -> ::core::option::Option<&::edge_toolkit::config::OtlpConfig> {
                self.otlp.as_ref()
            }
        }
    };
}

/// Shared `RUNNER_*` settings for both native runners.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RunnerConfig {
    /// Module to run, from `RUNNER_MODULE` (required).
    pub module: String,
    /// Optional wall-clock timeout, from `RUNNER_TIMEOUT` (e.g. `120s`, `3m`); `None` runs without a timeout.
    #[serde(default, with = "humantime_serde")]
    pub timeout: Option<Duration>,
}

/// Default time [`crate::connect_and_register`] waits for `et-connect-ack`.
pub const DEFAULT_CONNECT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared `WS_*` settings for both native runners.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct WsConfig {
    /// ws-server URL, from `WS_SERVER_URL`; defaults to the local insecure port.
    #[serde_inline_default(format!("ws://localhost:{}/ws", Services::InsecureWebSocketServer.port()))]
    pub server_url: String,

    /// How long [`crate::connect_and_register`] waits for the server's `et-connect-ack`.
    ///
    /// Read from `WS_CONNECT_ACK_TIMEOUT` as a humantime duration (e.g. `5s`, `500ms`). Unset defaults to 5s;
    /// `none`/`off`/`disabled` waits forever (retry until the server answers).
    #[serde(
        default = "default_connect_ack_timeout",
        deserialize_with = "edge_toolkit::config::deserialize_optional_humantime"
    )]
    pub connect_ack_timeout: Option<Duration>,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "serde default fn must return the field type Option<Duration>; the default is always Some"
)]
const fn default_connect_ack_timeout() -> Option<Duration> {
    Some(DEFAULT_CONNECT_ACK_TIMEOUT)
}
