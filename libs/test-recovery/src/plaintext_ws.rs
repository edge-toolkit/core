//! Plaintext WebSocket URLs the recovery supervisor's tests dial or validate.
//!
//! This module exists to be one file: it is the only place in the supervisor's tests that writes a `ws://` scheme,
//! and it is excluded from Codacy in `.codacy.yaml`. Codacy's "Insecure WebSocket Detected" pattern fires on the
//! literal and cannot be suppressed per line, since its in-repo config takes path excludes only. The tests genuinely
//! need plaintext: they talk to an in-process hub on loopback, or check that a cluster-internal plaintext URL is
//! accepted, and neither opens a connection that leaves the host. Isolating the literal here keeps every test that
//! uses it under full analysis.

/// The hub's WebSocket URL at `authority`, a `host:port` pair, over plaintext.
#[must_use]
pub fn plaintext_ws_url(authority: impl core::fmt::Display) -> String {
    format!("ws://{authority}/ws")
}
