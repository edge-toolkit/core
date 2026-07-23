//! Stage the browser tests' image fixture (the ws-server page favicon) into `OUT_DIR`.
//!
//! The `show_image` browser tests embed a real PNG via `include_bytes!`, and the repo's
//! `no-relative-path-literal` rule (rightly) forbids reaching it through a `../..` literal. Copying it here,
//! anchored on `et_path::find_project_root_from_manifest()`, keeps the checked-in favicon as the single
//! source of truth while giving the macro a stable `OUT_DIR` path to include.

use std::path::PathBuf;

#[expect(
    clippy::unwrap_used,
    reason = "build script: failing the build loudly is exactly right when the fixture can't stage"
)]
fn main() {
    let favicon = et_path::find_project_root_from_manifest().join("services/ws-server/static/favicon.png");
    println!("cargo:rerun-if-changed={}", favicon.display());

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let _bytes_copied = fs_err::copy(&favicon, out_dir.join("favicon.png")).unwrap();
}
