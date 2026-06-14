//! Emit `ET_WIT_DIR` (absolute path to the shared WIT directory) so the
//! `wit_bindgen::generate!` invocation in `src/lib.rs` locates it via `env!`,
//! instead of a `..`-relative path that hardcodes this crate's depth below the
//! repository root.

fn main() {
    let wit_dir = et_path::find_project_root_from_manifest().join("generated/specs/wit");
    println!("cargo:rustc-env=ET_WIT_DIR={}", wit_dir.display());
    println!("cargo:rerun-if-changed=build.rs");
}
