//! `/health` and `/info` route handlers, carrying the `#[utoipa::path]` annotations. `/health` answers with
//! et-ws-service's `HealthResponse`, which a board's hub answers with too.
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
use serde::{Deserialize, Serialize};

use crate::built_info;

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

/// How the answering server was built.
///
/// Returned by `GET /info`, so an operator can tell which release, toolchain and feature set a hub is running.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi-spec", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct BuildInfo {
    /// The crate the server was built from.
    pub name: String,
    /// Its version.
    pub version: String,
    /// The `rustc --version` that compiled it.
    pub rustc: String,
    /// The target triple it was compiled for.
    pub target: String,
    /// The cargo profile, `debug` or `release`.
    pub profile: String,
    /// The crate features it was built with, which name the services it can mount.
    pub features: Vec<String>,
}

impl BuildInfo {
    /// The build of this binary.
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: built_info::PKG_NAME.to_owned(),
            version: built_info::PKG_VERSION.to_owned(),
            rustc: built_info::RUSTC_VERSION.to_owned(),
            target: built_info::TARGET.to_owned(),
            profile: built_info::PROFILE.to_owned(),
            features: built_info::FEATURES_LOWERCASE
                .iter()
                .map(|&feature| feature.to_owned())
                .collect(),
        }
    }
}

/// Build information.
///
/// Returns the release, toolchain, target and features the server was built with.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        get,
        path = "/info",
        tag = "info",
        responses((status = 200, description = "How the server was built", body = BuildInfo))
    )
)]
pub async fn info() -> HttpResponse {
    HttpResponse::Ok().json(BuildInfo::current())
}
