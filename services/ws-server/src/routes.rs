//! `/health` route handler and its response schema, carrying the
//! `#[utoipa::path]` and `utoipa::ToSchema` annotations.
//!
//! Lives in its own file so the file-level
//! `#![expect(clippy::exhaustive_structs)]` (active under the
//! `openapi-spec` feature) is scoped only to where utoipa's derives
//! emit `pub struct`s without `#[non_exhaustive]` -- neither location
//! can be silenced via a function- or item-level `#[expect]`.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa emits sibling pub structs without #[non_exhaustive]; only file scope can silence the lint"
    )
)]

use actix_web::HttpResponse;
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

/// Liveness probe.
///
/// Returns a small JSON document identifying the service so external
/// monitors can confirm the server is reachable and serving requests.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        get,
        path = "/health",
        tag = "health",
        responses((status = 200, description = "Server is up", body = HealthResponse))
    )
)]
pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(HealthResponse {
        status: "healthy".to_string(),
        service: "ws-server".to_string(),
    })
}
