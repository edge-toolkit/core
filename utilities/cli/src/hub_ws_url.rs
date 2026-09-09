//! WebSocket URLs a generated deployment points a runner's `WS_SERVER_URL` at.
//!
//! This module exists to be one file rather than because the two functions below need their own home: it is the
//! only place in `et-cli` that writes a `ws://` scheme, and it is excluded from Codacy in `.codacy.yaml`. Codacy's
//! "Insecure WebSocket Detected" pattern fires on the literal, cannot be suppressed per line -- its in-repo config
//! takes path excludes only -- and cannot be fixed, because a generated deployment genuinely does speak plaintext
//! WebSocket between its components. Isolating the literal here keeps every other line of the generators, which is
//! nearly all of `et-cli`, under full analysis; excluding `deployment_types/k3s.rs` or `lib.rs` instead would have
//! dropped hundreds of analysed lines to silence two.
//!
//! Plaintext is a real limitation and not a considered preference. Serving `wss://` would need the hub's
//! self-signed certificate to carry the names a client actually dials -- it is generated for `localhost`,
//! `127.0.0.1` and `::1` -- and would need the runners to trust it, which `libs/ws-runner-common` has no
//! configuration for. Both are prerequisites nobody has built, so until they exist there is nothing to point a
//! `wss://` URL at.

use edge_toolkit::ports::Services;

/// Name the in-cluster `Service` that fronts the hub.
///
/// Duplicated from the k3s generator's own constant would be one definition too many, so the URL builder that
/// needs it owns the name and the generator names its objects from here.
pub const HUB_SERVICE: &str = "ws-server";

/// WebSocket URL for a runner sharing a host with the hub, which is how mise and compose arrange it.
#[must_use]
pub fn hub_ws_url() -> String {
    format!("ws://localhost:{}/ws", Services::InsecureWebSocketServer.port())
}

/// WebSocket URL for a runner reaching the hub across a cluster, where `localhost` is the runner's own pod.
#[must_use]
pub fn hub_service_ws_url() -> String {
    format!("ws://{HUB_SERVICE}:{}/ws", Services::InsecureWebSocketServer.port())
}
