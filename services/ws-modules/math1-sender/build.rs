//! Emit `ET_MATH1_INPUT_PATH` (absolute path to the canonical math1 input JSON) so `include_str!`
//! in `src/lib.rs` embeds the same bytes every math1 test harness injects, without a `..`-relative
//! path that hardcodes this crate's depth below the repository root.

fn main() {
    let input = et_path::find_project_root_from_manifest().join("services/ws-test-server/data/math1-input.json");
    println!("cargo:rustc-env=ET_MATH1_INPUT_PATH={}", input.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", input.display());
}
