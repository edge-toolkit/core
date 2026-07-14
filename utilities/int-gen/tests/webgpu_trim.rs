//! Exercises the `strip_webgpu` WIT-trimming pipeline on the committed compute-subset fixture.
//!
//! The upstream fetch (`run` / `fetch_*`) is network-bound, but the parse-filter-reemit core is pure, so we
//! drive it directly against the trimmed `webgpu.wit` under `generated/` -- a self-contained `wasi:webgpu`
//! package with records, variants, resources, enums and flags, which walks every arm of `mutate_interface`
//! and `collect_type_refs`.
#![cfg(test)]

use et_int_gen::wit::upstream::strip_webgpu;

#[test]
fn strips_webgpu_wit_and_reemits_the_package() {
    // Anchor to the repo root (no relative-path literal) and read the committed fixture at runtime.
    let wit_path = edge_toolkit::config::get_project_root().join("generated/specs/wit/deps/wasi-webgpu/webgpu.wit");
    let raw = fs_err::read_to_string(&wit_path).unwrap();

    let out = strip_webgpu(&raw).unwrap();
    assert!(
        out.contains("package wasi:webgpu"),
        "re-emitted WIT should still declare the wasi:webgpu package, got:\n{out}"
    );
    // The trimmer keeps the compute resources (e.g. gpu-device) while dropping cross-package glue.
    assert!(
        out.contains("resource gpu-device"),
        "expected the gpu-device resource to survive, got:\n{out}"
    );
}
