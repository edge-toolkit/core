//! `/health` route handler carrying `#[utoipa::path]`.
//!
//! Lives in its own file so the file-level `#![expect(clippy::exhaustive_structs)]`
//! (active under the `openapi-spec` feature) is scoped only to the place
//! where the lint can fire — `utoipa::path` emits `__path_*` types as
//! siblings of each annotated function, and the lint cannot be reached
//! through an `#[expect]` on the function itself.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa::path emits `__path_*` types without #[non_exhaustive]; scoped only to this openapi module"
    )
)]

use actix_web::HttpResponse;

use crate::HealthResponse;

/// Liveness probe. Returns a small JSON document identifying the service
/// so external monitors can confirm the server is reachable and serving
/// requests.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        get,
        path = "/health",
        responses((status = 200, description = "Server is up", body = HealthResponse))
    )
)]
pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(HealthResponse {
        status: "healthy".to_string(),
        service: "ws-server".to_string(),
    })
}
