#![cfg(test)]

use et_cli::generate_module_package_json;
use fs_err as fs;
use serde_json::Value;
use tempfile::tempdir;

/// Build a module directory holding `manifest` plus an empty `entry` under `pkg/`, and generate its package.json.
///
/// Every successful case below needs exactly this scaffolding -- a temp root, a `pkg/` directory, an entry file
/// for the resolver to find, the manifest written out -- and differs only in those values and in what it
/// asserts, so it lives here once. `extras` covers the cases that need one more file in the tree, each path
/// relative to the module directory with its parents created. The parsed JSON is returned rather than the path
/// because the temp root is dropped on the way out, taking the generated file with it.
fn generated_package(manifest_name: &str, manifest: &str, entry: &str, extras: &[(&str, &str)]) -> Value {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join(entry), "").unwrap();
    fs::write(module_dir.join(manifest_name), manifest).unwrap();
    for (path, contents) in extras {
        let target = module_dir.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    let output_path = generate_module_package_json(module_dir).unwrap();
    serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap()
}

#[test]
fn module_package_json_generates_from_pyproject_metadata() {
    let package = generated_package(
        "pyproject.toml",
        r#"[project]
name = "et-ws-python-module"
version = "0.1.0"
description = "Python module"
license = "Apache-2.0"

[tool.ws-module.dependencies]
et-model-face1 = "*"
"#,
        "et_ws_python_module.js",
        &[],
    );

    assert_eq!(package["name"], "et-ws-python-module");
    assert_eq!(package["type"], "module");
    assert_eq!(package["description"], "Python module");
    assert_eq!(package["version"], "0.1.0");
    assert_eq!(package["license"], "Apache-2.0");
    assert_eq!(package["main"], "et_ws_python_module.js");
    assert_eq!(package["dependencies"]["et-model-face1"], "*");
}

#[test]
fn module_package_json_derives_wasi_main_from_crate_name() {
    let package = generated_package(
        "Cargo.toml",
        r#"[package]
name = "et-ws-wasi-demo"
version = "0.1.0"
edition = "2024"

[dependencies]
wit-bindgen = "0.57"
"#,
        "et_ws_wasi_demo.wasm",
        &[],
    );

    assert_eq!(package["main"], "et_ws_wasi_demo.wasm");
}

/// A guest that reaches the component bindings through `et-wasi-guest` is still a component.
///
/// The kind is detected from the manifest text, and the marker used to be `wit-bindgen` alone. Once the shared
/// guest crate generated the bindings for everyone, no guest named `wit-bindgen` any more, every WASI module
/// silently became a browser module, and the entry lookup went hunting for a `.js` that is never built --
/// `UnresolvedMainFile { ..., ext: "js" }` for all four of them, with nothing in the crate itself changed.
#[test]
fn module_package_json_derives_wasi_main_via_the_shared_guest_crate() {
    let package = generated_package(
        "Cargo.toml",
        r#"[package]
name = "et-ws-wasi-shared"
version = "0.1.0"
edition = "2024"

[target.'cfg(target_os = "wasi")'.dependencies]
et-wasi-guest = { workspace = true }
"#,
        "et_ws_wasi_shared.wasm",
        &[],
    );

    assert_eq!(package["main"], "et_ws_wasi_shared.wasm");
}

#[test]
fn module_package_json_respects_main_override() {
    let package = generated_package(
        "Cargo.toml",
        r#"[package]
name = "et-ws-override-module"
version = "0.1.0"
edition = "2024"

[package.metadata.ws-module]
main = "custom_entry.wasm"
"#,
        "custom_entry.wasm",
        &[],
    );

    assert_eq!(package["main"], "custom_entry.wasm");
}

#[test]
fn module_package_json_derives_wasi_main_from_pyproject() {
    let package = generated_package(
        "pyproject.toml",
        r#"[project]
name = "et-ws-wasi-pydemo"
version = "0.1.0"
description = "WASI Python demo"
"#,
        "et_ws_wasi_pydemo.wasm",
        // A Python module's kind is read from the presence of the componentize-py bindings directory rather
        // than from its manifest, so what this file holds does not matter -- only that the directory exists.
        &[("wit_world/bindings.py", "")],
    );

    assert_eq!(package["main"], "et_ws_wasi_pydemo.wasm");
}

#[test]
fn module_package_json_merges_cargo_ws_module_dependencies() {
    let package = generated_package(
        "Cargo.toml",
        r#"[package]
name = "et-ws-rust-module"
version = "0.1.0"
edition = "2024"

[package.metadata.ws-module.dependencies]
et-model-har-motion1 = "*"
"#,
        "et_ws_rust_module.js",
        // A package.json already in pkg/ is merged into rather than replaced, so its entries have to survive.
        &[(
            "pkg/package.json",
            r#"{
  "type": "module",
  "main": "et_ws_rust_module.js",
  "dependencies": {
    "existing-package": "1.0.0"
  }
}
"#,
        )],
    );

    assert_eq!(package["name"], "et-ws-rust-module");
    assert_eq!(package["type"], "module");
    assert_eq!(package["main"], "et_ws_rust_module.js");
    assert_eq!(package["dependencies"]["existing-package"], "1.0.0");
    assert_eq!(package["dependencies"]["et-model-har-motion1"], "*");
}

#[test]
fn module_package_json_fails_when_main_missing() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    fs::create_dir_all(module_dir.join("pkg")).unwrap();
    fs::write(
        module_dir.join("Cargo.toml"),
        r#"[package]
name = "et-ws-missing-module"
version = "0.1.0"
edition = "2024"
"#,
    )
    .unwrap();

    let error = generate_module_package_json(module_dir).unwrap_err();
    assert!(error.to_string().contains("No main file"), "unexpected error: {error}");
}
