//! Port allocation.

/// Define each services port.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Services {
    /// OTLP collector.
    OtlpCollector,

    /// Insecure WebSocket server.
    InsecureWebSocketServer,
    /// Secure WebSocket server.
    SecureWebSocketServer,
}

use Services::{InsecureWebSocketServer, OtlpCollector, SecureWebSocketServer};

impl Services {
    /// Get the allocation port for the service.
    #[must_use]
    pub const fn port(&self) -> u16 {
        match self {
            // OpenObserve specific http port
            OtlpCollector => 5080,

            InsecureWebSocketServer => 8080,
            SecureWebSocketServer => 8443,
        }
    }
}
