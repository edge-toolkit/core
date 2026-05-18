//! Helpers and constants shared by the native ws-server agent runners
//! (`et-ws-wasi-runner`, `et-ws-web-runner`, `et-ws-pyo3-runner`).
//!
//! Bootstrap helpers talk to the ws-server REST surface to set up a module:
//! derive the HTTP base from the WebSocket URL, drain streamed responses, and
//! read the `main` entry from `package.json`. Connection helpers cover the
//! shared agent-loop timing: the connect-ack timeout and the keepalive
//! heartbeat (the server times out idle connections and never pings clients).
//! These were duplicated across the runner crates; one implementation here
//! keeps them in sync with the server.

// `BootstrapError` is large because `et_rest_client::Error<()>` carries an
// inline `reqwest::Response` (~136 B). Boxing would cost a `From` impl per
// variant; not worth it for these one-shot runner helpers (the two runner
// crates carry the same expectation for the same reason).
#![expect(
    clippy::result_large_err,
    reason = "et_rest_client::Error<()> dominates the footprint; boxing would force per-variant From impls"
)]

use std::time::{Duration, SystemTime};

use edge_toolkit::ws::{ClientMessage, ConnectStatus, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use retry_policies::policies::ExponentialBackoff;
use retry_policies::{RetryDecision, RetryPolicy};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};

pub mod config;

/// A live websocket to the ws-server that has completed the et-connect handshake.
pub type RegisteredSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Lower bound on the connect-ack retry interval. Stops a small configured
/// timeout from collapsing the backoff to near-zero (which would hammer the
/// server); also the interval used when the timeout is disabled (retry forever).
const MIN_RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Upper bound on the connect-ack retry interval, so a long or disabled timeout
/// doesn't let the backoff crawl out to retry-policies' multi-minute default.
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// How often a runner pings the ws-server to stay connected.
///
/// The server closes connections idle longer than its connection timeout
/// (`WS_CONNECTION_TIMEOUT`, default 15s; see `services/ws/src/lib.rs`) and
/// never pings clients itself, so an agent that only waits for inbound frames
/// must ping well inside that window.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Build a heartbeat ticker firing every [`HEARTBEAT_INTERVAL`].
///
/// The immediate first tick is consumed so the first heartbeat fires one
/// interval after connect (not instantly), and missed ticks are delayed rather
/// than bursting to catch up. Drive it with `interval.tick().await` and send a
/// WebSocket ping each tick.
pub async fn heartbeat_interval() -> tokio::time::Interval {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let _first: tokio::time::Instant = interval.tick().await;
    interval
}

/// Errors from [`connect_and_register`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConnectError {
    /// Opening, sending on, or reading the websocket failed.
    #[error("websocket error during connect/register: {0}")]
    WebSocket(#[from] tungstenite::Error),

    /// The `et-connect` frame could not be serialised.
    #[error("failed to serialize the et-connect frame: {0}")]
    Serialize(#[from] serde_json::Error),

    /// No `et-connect-ack` arrived within a single attempt's budget.
    #[error("no et-connect-ack within {0:?}")]
    AckTimeout(Duration),

    /// The connection closed before any `et-connect-ack` arrived.
    #[error("connection closed before et-connect-ack")]
    ConnectionClosed,
}

/// Human label for a [`ConnectStatus`], for log lines.
#[must_use]
pub const fn connect_status_label(status: &ConnectStatus) -> &'static str {
    match *status {
        ConnectStatus::Assigned => "assigned",
        ConnectStatus::Reconnected => "reconnected",
    }
}

/// Connect to the ws-server and complete the `et-connect` handshake.
///
/// Opens the websocket, sends `et-connect` (requesting `requested_agent_id` if
/// given), and waits for `et-connect-ack`, returning the live socket plus the
/// assigned id and status.
///
/// The wait is a retry loop: the whole attempt (connect + register) is retried
/// with exponential backoff until it succeeds or `ack_timeout` elapses, so a
/// runner started before the ws-server simply waits for it to come up.
/// `ack_timeout = None` retries forever. The backoff interval is floored so a
/// small timeout never degrades into a busy-loop.
pub async fn connect_and_register(
    ws_url: &str,
    requested_agent_id: Option<String>,
    ack_timeout: Option<Duration>,
) -> Result<(RegisteredSocket, String, ConnectStatus), ConnectError> {
    let policy = backoff_for_timeout(ack_timeout);
    let started_at = SystemTime::now();
    let mut n_past_retries = 0_u32;
    loop {
        let budget = attempt_budget(ack_timeout, started_at);
        match register_once(ws_url, requested_agent_id.clone(), budget).await {
            Ok(registered) => return Ok(registered),
            Err(err) => match policy.should_retry(started_at, n_past_retries) {
                RetryDecision::Retry { execute_after } => {
                    let wait = execute_after.duration_since(SystemTime::now()).unwrap_or_default();
                    tracing::warn!(attempt = n_past_retries.saturating_add(1), error = %err, retry_in = ?wait,
                        "connect/register attempt failed; retrying");
                    tokio::time::sleep(wait).await;
                    n_past_retries = n_past_retries.saturating_add(1);
                }
                RetryDecision::DoNotRetry => return Err(err),
            },
        }
    }
}

/// Smart timeout -> retry policy. `Some(total)` bounds total retry time to
/// `total`; `None` retries forever. The backoff floor is a fraction of the
/// total but never below [`MIN_RETRY_INTERVAL`] (so a small timeout doesn't
/// collapse the interval to near-zero and hammer the server) nor above the cap.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of connect_and_register; kept separate for readability and future reuse"
)]
fn backoff_for_timeout(timeout: Option<Duration>) -> Box<dyn RetryPolicy + Send + Sync> {
    let retry_min = timeout.map_or(MIN_RETRY_INTERVAL, |total| {
        total
            .checked_div(8)
            .unwrap_or(MIN_RETRY_INTERVAL)
            .clamp(MIN_RETRY_INTERVAL, MAX_RETRY_INTERVAL)
    });
    let builder = ExponentialBackoff::builder().retry_bounds(retry_min, MAX_RETRY_INTERVAL);
    match timeout {
        Some(total) => Box::new(builder.build_with_total_retry_duration(total)),
        None => Box::new(builder.build_with_max_retries(u32::MAX)),
    }
}

/// Per-attempt budget: the time left before the total deadline, clamped so one
/// attempt can't overrun the cap and always gets a minimum window to connect.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of connect_and_register; kept separate for readability and future reuse"
)]
fn attempt_budget(ack_timeout: Option<Duration>, started_at: SystemTime) -> Duration {
    ack_timeout.map_or(MAX_RETRY_INTERVAL, |total| {
        total
            .saturating_sub(started_at.elapsed().unwrap_or_default())
            .clamp(MIN_RETRY_INTERVAL, MAX_RETRY_INTERVAL)
    })
}

/// One connect + send-`et-connect` + await-`et-connect-ack`, bounded by `budget`.
#[expect(
    clippy::single_call_fn,
    reason = "one attempt of the connect_and_register retry loop; separated for readability"
)]
async fn register_once(
    ws_url: &str,
    requested_agent_id: Option<String>,
    budget: Duration,
) -> Result<(RegisteredSocket, String, ConnectStatus), ConnectError> {
    let attempt = async {
        let (mut socket, _response) = connect_async(ws_url).await?;
        let connect = serde_json::to_string(&ClientMessage::Connect {
            agent_id: requested_agent_id,
        })?;
        socket.send(tungstenite::Message::Text(connect)).await?;
        while let Some(frame) = socket.next().await {
            let tungstenite::Message::Text(text) = frame? else {
                continue;
            };
            match ServerMessage::from_text_frame(&text) {
                Ok(ServerMessage::ConnectAck { agent_id, status }) => return Ok((socket, agent_id, status)),
                Ok(_) => {}
                Err(err) => tracing::warn!(error = %err, "ignoring undecodable et-* frame during handshake"),
            }
        }
        Err(ConnectError::ConnectionClosed)
    };
    tokio::time::timeout(budget, attempt)
        .await
        .unwrap_or_else(|_elapsed| Err(ConnectError::AckTimeout(budget)))
}

/// Errors produced while bootstrapping a module from the ws-server.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BootstrapError {
    /// `ws_url` was not a `ws://` / `wss://` URL, so no HTTP base could be derived.
    #[error("could not derive HTTP base from WS_SERVER_URL={ws_url}")]
    InvalidWsUrl { ws_url: String },

    /// A REST request to the ws-server failed.
    #[error(transparent)]
    Rest(#[from] et_rest_client::Error<()>),

    /// Streaming a response body chunk from the ws-server failed.
    ///
    /// `ByteStream` chunks surface as `reqwest::Error`, distinct from the typed `Rest` arm.
    #[error(transparent)]
    Stream(#[from] reqwest::Error),

    /// A module's `package.json` was not valid JSON.
    #[error(transparent)]
    PackageJsonInvalid(#[from] serde_path_to_error::Error<serde_json::Error>),

    /// A module's `package.json` parsed but had no `main` field.
    #[error("module {module} package.json missing `main` field")]
    PackageJsonMissingMain { module: String },
}

/// Derive a module's HTTP base URL from the ws-server WebSocket URL.
///
/// Maps the scheme (`ws://` -> `http://`, `wss://` -> `https://`) and strips a
/// trailing `/ws` path, e.g. `ws://host:8080/ws` -> `http://host:8080`.
///
/// # Errors
/// Returns [`BootstrapError::InvalidWsUrl`] if `ws_url` is not a `ws://` /
/// `wss://` URL.
pub fn derive_http_base(ws_url: &str) -> Result<String, BootstrapError> {
    let (scheme, rest) = if let Some(suffix) = ws_url.strip_prefix("wss://") {
        ("https", suffix)
    } else if let Some(suffix) = ws_url.strip_prefix("ws://") {
        ("http", suffix)
    } else {
        return Err(BootstrapError::InvalidWsUrl {
            ws_url: ws_url.to_string(),
        });
    };
    let host_port = rest.strip_suffix("/ws").unwrap_or(rest);
    Ok(format!("{scheme}://{host_port}"))
}

/// Drain a progenitor `ByteStream` into a `Vec<u8>`.
///
/// # Errors
/// Returns [`BootstrapError::Stream`] if downloading a chunk fails.
pub async fn collect_byte_stream(mut stream: et_rest_client::ByteStream) -> Result<Vec<u8>, BootstrapError> {
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
    }
    Ok(buf)
}

/// Read a module's `main` entry-point filename from its `package.json`.
///
/// Fetches `package.json` from the ws-server and returns its `main` field,
/// which names the file the runner downloads next (a WASI component for the
/// wasi runner, a JS entry for the web runner).
///
/// # Errors
/// Returns [`BootstrapError::Rest`] / [`BootstrapError::Stream`] if the fetch
/// fails, [`BootstrapError::PackageJsonInvalid`] if the body is not valid JSON,
/// or [`BootstrapError::PackageJsonMissingMain`] if it has no `main` field.
#[tracing::instrument(name = "fetch_package_json", skip(client), err)]
pub async fn fetch_main_field(client: &et_rest_client::Client, module_name: &str) -> Result<String, BootstrapError> {
    let response = client.get_module_file(module_name, "package.json").await?;
    let bytes = collect_byte_stream(response.into_inner()).await?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let pkg: serde_json::Value = serde_path_to_error::deserialize(&mut deserializer)?;
    pkg.get("main")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| BootstrapError::PackageJsonMissingMain {
            module: module_name.to_string(),
        })
}
