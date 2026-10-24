//! Port allocation.

use Services::{
    Backplane, BackplaneMonitor, InsecureWebSocketServer, OtlpCollector, SecureWebSocketServer, SharedStorage,
};

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

    /// NATS client port of the backplane several hubs share.
    Backplane,
    /// NATS monitoring port of the backplane, which serves its health check.
    BackplaneMonitor,
    /// S3 endpoint of the object store several hubs share.
    SharedStorage,
}

impl Services {
    /// Get the allocation port for the service.
    #[must_use]
    pub const fn port(self) -> u16 {
        match self {
            // OpenObserve specific http port
            OtlpCollector => 5080,

            InsecureWebSocketServer => 8080,
            SecureWebSocketServer => 8443,

            // NATS's and rustfs's own defaults
            Backplane => 4222,
            BackplaneMonitor => 8222,
            SharedStorage => 9000,
        }
    }
}
