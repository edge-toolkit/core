#![cfg(test)]

use std::fs;

use et_cli::generate_module_package_json;
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn module_package_json_generates_from_pyproject_metadata() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    fs::write(
        module_dir.join("pyproject.toml"),
        r#"[project]
name = "et-ws-python-module"
version = "0.1.0"
description = "Python module"
license = "Apache-2.0"

[tool.ws-module]
js-main = "python_module.js"

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
    assert_eq!(package["main"], "python_module.js");
    assert_eq!(package["dependencies"]["et-model-face1"], "*");
}

#[test]
fn module_package_json_merges_cargo_ws_module_dependencies() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path();
    let package_dir = module_dir.join("pkg");
    fs::create_dir_all(&package_dir).unwrap();
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
    assert_eq!(package["dependencies"]["existing-package"], "1.0.0");
    assert_eq!(package["dependencies"]["et-model-har-motion1"], "*");
}
