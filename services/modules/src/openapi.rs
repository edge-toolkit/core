//! `#[utoipa::path]`-annotated route handlers.
//!
//! Lives in its own file so the file-level
//! `#![expect(clippy::exhaustive_structs)]` (active under the `openapi-spec`
//! feature) is scoped only to the place where the lint can fire —
//! `utoipa::path` emits `__path_*` types as siblings of each annotated
//! function, and the lint cannot be reached through an `#[expect]` on the
//! function itself.
//!
//! `list_modules_handler` is the live actix-web route handler.
//! `get_module_file` is a stub used only for `#[utoipa::path]` attachment;
//! the request is served by an `actix_files::Files` mount registered in
//! `crate::configure`.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa::path emits `__path_*` types without #[non_exhaustive]; scoped only to this openapi module"
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
        responses((status = 200, description = "Names of available modules", body = Vec<String>))
    )
)]
pub async fn list_modules_handler(config: web::Data<ModulesConfig>) -> HttpResponse {
    let names: Vec<String> = list_modules(&config).into_iter().map(|(name, _)| name).collect();
    HttpResponse::Ok().json(names)
}

/// Fetch a file from a module's bundled static assets. `path` is resolved
/// relative to the module's bundle root; an unknown module or missing file
/// returns 404.
#[cfg(feature = "openapi-spec")]
#[utoipa::path(
    get,
    path = "/modules/{name}/{path}",
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
    HttpResponse::NotImplemented().finish()
}
