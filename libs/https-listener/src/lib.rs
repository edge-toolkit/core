//! What an edge-toolkit server needs to open an HTTPS listener a browser on another device can reach.
//!
//! [`net`] finds the host's LAN addresses worth advertising, and [`tls`] loads or generates the certificate a listener
//! serves, a self-signed one covering those addresses as well as the loopback names.
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

pub mod net;
#[cfg(feature = "tls")]
pub mod tls;
