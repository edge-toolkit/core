//! Environment-derived configuration shared by both native runners.
//!
//! Each runner deserialises its own top-level `Config` from the process
//! environment via `serde-env`, nesting these structs under `runner` / `ws`
//! fields. With serde-env's `_`-segmented mapping that puts every `RUNNER_*`
//! var under [`RunnerConfig`] and every `WS_*` var under [`WsConfig`], so the
//! two runners parse the common variables identically.

use std::time::Duration;

use edge_toolkit::ports::Services;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

/// `RUNNER_*` settings shared by both native runners.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RunnerConfig {
    /// Module to run, from `RUNNER_MODULE` (required).
    pub module: String,
    /// Optional wall-clock timeout, from `RUNNER_TIMEOUT` (e.g. `120s`, `3m`);
    /// `None` runs without a timeout.
    #[serde(default, with = "humantime_serde")]
    pub timeout: Option<Duration>,
}

/// Default time [`crate::connect_and_register`] waits for `et-connect-ack`.
pub const DEFAULT_CONNECT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// `WS_*` settings shared by both native runners.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct WsConfig {
    /// ws-server URL, from `WS_SERVER_URL`; defaults to the local insecure port.
    #[serde_inline_default(format!("ws://localhost:{}/ws", Services::InsecureWebSocketServer.port()))]
    pub server_url: String,

    /// How long [`crate::connect_and_register`] waits for the server's
    /// `et-connect-ack`, from `WS_CONNECT_ACK_TIMEOUT` as a humantime duration
    /// (e.g. `5s`, `500ms`). Unset defaults to 5s; `none`/`off`/`disabled` waits
    /// forever (retry until the server answers).
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
