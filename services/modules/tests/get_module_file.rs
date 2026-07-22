//! Covers `get_module_file`: a fake stub that exists only to host the utoipa route annotation for
//! `et-int-gen`, since the real `GET /modules/{name}/{path}` route is served by the `actix_files::Files`
//! mounts `configure` registers, not by Rust code. Pins its stub response so a future change to it is
//! deliberate, not accidental.
#![cfg(test)]
#![cfg(feature = "openapi-spec")]

use actix_web::http::StatusCode;
use et_modules_service::routes::get_module_file;

#[test]
fn returns_not_implemented() {
    let resp = get_module_file();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}
