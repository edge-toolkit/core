//! The answer to `GET /health`, which every server of the hub gives, on an operating system or on a board.

#[cfg(not(feature = "std"))]
use alloc::string::{String, ToString as _};

use serde::{Deserialize, Serialize};

/// Server liveness probe response.
///
/// Returned by `GET /health`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi-spec", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

impl HealthResponse {
    /// What a server that is up answers, whichever server it is.
    #[must_use]
    pub fn healthy() -> Self {
        Self {
            status: "healthy".to_string(),
            service: "ws-server".to_string(),
        }
    }
}
