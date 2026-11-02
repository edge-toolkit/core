//! `/health` route handler, carrying the `#[utoipa::path]` annotation. Its response schema is et-ws-service's
//! `HealthResponse`, which a board's hub answers with too.
//!
//! Lives in its own file so the file-level `#![expect(clippy::exhaustive_structs)]` (active under the `openapi-spec`
//! feature) is scoped only to where utoipa's path macro emits a `pub struct` without `#[non_exhaustive]`, which no
//! item-level `#[expect]` can reach.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa emits a sibling pub struct without #[non_exhaustive]; only file scope can silence the lint"
    )
)]

use actix_web::HttpResponse;
use et_ws_service::health::HealthResponse;

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
    HttpResponse::Ok().json(HealthResponse::healthy())
}
