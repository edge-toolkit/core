//! Modules HTTP routes carrying `#[utoipa::path]` annotations.
//!
//! `list_modules_handler` is the live `/modules/` route handler.
//! `get_module_file` is a fake stub -- the per-module GET routes are
//! served by `actix_files::Files` mounts registered in
//! [`crate::configure`], not by Rust; this function exists only to
//! host the `#[utoipa::path]` annotation.
//!
//! The file-level `#![expect(clippy::exhaustive_structs)]` (active
//! under `openapi-spec`) is scoped here because utoipa's derives emit
//! `pub struct`s without `#[non_exhaustive]` -- and the lint locations
//! aren't reachable through a function- or item-level `#[expect]`.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa emits sibling pub structs without #[non_exhaustive]; only file scope can silence the lint"
    )
)]

use actix_web::{HttpResponse, web};

use crate::{ModulesConfig, list_modules};

/// List the names of every module the server is currently serving.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        get,
        path = "/modules/",
        tag = "modules",
        responses((status = 200, description = "Names of available modules", body = Vec<String>))
    )
)]
pub async fn list_modules_handler(config: web::Data<ModulesConfig>) -> HttpResponse {
    let names: Vec<String> = list_modules(&config).into_iter().map(|(name, _)| name).collect();
    HttpResponse::Ok().json(names)
}

/// Fetch a file from a module's bundled static assets.
///
/// `path` is resolved relative to the module's bundle root; an unknown
/// module or missing file returns 404.
#[cfg(feature = "openapi-spec")]
#[utoipa::path(
    get,
    path = "/modules/{name}/{path}",
    tag = "modules",
    params(
        ("name" = String, Path, description = "Module name"),
        ("path" = String, Path, description = "Path of the file within the module bundle")
    ),
    responses(
        (status = 200, description = "Static module asset", content_type = "application/octet-stream"),
        (status = 404, description = "No such module or file")
    )
)]
#[must_use]
pub fn get_module_file() -> HttpResponse {
    // Fake handler -- the GET route is actually served by the per-module
    // `actix_files::Files` mounts registered in `crate::configure`; this
    // stub exists only to host the `#[utoipa::path]` annotation so
    // `et-int-gen` can include the route in `generated/specs/rest.yaml`.
    HttpResponse::NotImplemented().finish()
}
