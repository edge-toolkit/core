#![cfg(test)]

use et_cli::generate_module_package_json;
use fs_err as fs;
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn module_package_json_generates_from_pyproject_metadata() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("et_ws_python_module.js"), "").unwrap();
    fs::write(
        module_dir.join("pyproject.toml"),
        r#"[project]
name = "et-ws-python-module"
version = "0.1.0"
description = "Python module"
license = "Apache-2.0"

[tool.ws-module.dependencies]
et-model-face1 = "*"
"#,
    )
    .unwrap();

    let output_path = generate_module_package_json(module_dir).unwrap();
    let package: Value = serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap();

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
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("et_ws_wasi_demo.wasm"), "").unwrap();
    fs::write(
        module_dir.join("Cargo.toml"),
        r#"[package]
name = "et-ws-wasi-demo"
version = "0.1.0"
edition = "2024"

[dependencies]
wit-bindgen = "0.57"
"#,
    )
    .unwrap();

    let output_path = generate_module_package_json(module_dir).unwrap();
    let package: Value = serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap();

    assert_eq!(package["main"], "et_ws_wasi_demo.wasm");
}

#[test]
fn module_package_json_derives_wasi_main_from_pyproject() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::create_dir_all(module_dir.join("wit_world")).unwrap();
    fs::write(package_dir.join("et_ws_wasi_pydemo.wasm"), "").unwrap();
    fs::write(
        module_dir.join("pyproject.toml"),
        r#"[project]
name = "et-ws-wasi-pydemo"
version = "0.1.0"
description = "WASI Python demo"
"#,
    )
    .unwrap();

    let output_path = generate_module_package_json(module_dir).unwrap();
    let package: Value = serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap();

    assert_eq!(package["main"], "et_ws_wasi_pydemo.wasm");
}

#[test]
fn module_package_json_merges_cargo_ws_module_dependencies() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("et_ws_rust_module.js"), "").unwrap();
    fs::write(
        module_dir.join("Cargo.toml"),
        r#"[package]
name = "et-ws-rust-module"
version = "0.1.0"
edition = "2024"

[package.metadata.ws-module.dependencies]
et-model-har-motion1 = "*"
"#,
    )
    .unwrap();
    fs::write(
        package_dir.join("package.json"),
        r#"{
  "type": "module",
  "main": "et_ws_rust_module.js",
  "dependencies": {
    "existing-package": "1.0.0"
  }
}
"#,
    )
    .unwrap();

    let output_path = generate_module_package_json(module_dir).unwrap();
    let package: Value = serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap();

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

#[test]
fn module_package_json_respects_main_override() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("custom_entry.wasm"), "").unwrap();
    fs::write(
        module_dir.join("Cargo.toml"),
        r#"[package]
name = "et-ws-override-module"
version = "0.1.0"
edition = "2024"

[package.metadata.ws-module]
main = "custom_entry.wasm"
"#,
    )
    .unwrap();

    let output_path = generate_module_package_json(module_dir).unwrap();
    let package: Value = serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap();

    assert_eq!(package["main"], "custom_entry.wasm");
}
