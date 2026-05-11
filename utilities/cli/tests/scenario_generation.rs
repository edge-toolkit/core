#![cfg(test)]

use std::fs;

use et_cli::{
    DeploymentMode, DeploymentOptions, OutputType, docker_image_module_paths, generate_deployment,
    generate_deployment_with_options, module_package_json, regenerate_verification,
    regenerate_verification_with_options, scenario_module_paths,
};
use tempfile::tempdir;

#[test]
fn generate_deployment_rejects_unsupported_deployment_type() {
    let test_root = tempdir().unwrap();
    let input_dir = test_root.path().join("input");
    let output_dir = test_root.path().join("output");
    fs::create_dir_all(&input_dir).unwrap();

    let input_file = input_dir.join("cluster.yaml");
    fs::write(
        &input_file,
        r#"cluster_name: "test-cluster"
deployment_type: yaml
agents: []
"#,
    )
    .unwrap();

    let error = generate_deployment(&input_file, &output_dir, None).unwrap_err();
    assert!(error.to_string().contains("Unsupported deployment_type"));
}

#[test]
fn docker_image_module_paths_include_static_root_module() {
    let paths = docker_image_module_paths(&["face-detection".to_string()]).unwrap();

    assert_eq!(paths[0], "/app/services/ws-server/static");
    assert!(paths.contains(&"/app/services/ws-wasm-agent".to_string()));
    assert!(paths.contains(&"/app/data/model-modules/model-face1".to_string()));
    assert!(paths.contains(&"/app/node_modules/onnxruntime-web".to_string()));
    assert!(!paths.contains(&"/app/node_modules/pyodide".to_string()));
    assert!(paths.contains(&"/app/services/ws-modules/face-detection".to_string()));
}

#[test]
fn scenario_module_paths_include_selected_modules_and_dependencies() {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = project_root.join("services/ws-server");
    let paths = scenario_module_paths(&ws_server_dir, &["face-detection".to_string(), "har1".to_string()]).unwrap();

    assert_eq!(
        paths,
        vec![
            "static".to_string(),
            "../ws-wasm-agent".to_string(),
            "../ws-modules/face-detection".to_string(),
            "../ws-modules/har1".to_string(),
            "../../data/model-modules/model-face1".to_string(),
            "$(mise where npm:onnxruntime-web)/lib/node_modules/onnxruntime-web".to_string(),
            "../../data/model-modules/model-har-motion1".to_string(),
        ],
    );
    assert!(!paths.contains(&"../ws-modules".to_string()));
    assert!(!paths.contains(&"../ws-modules/data1".to_string()));
}

#[test]
fn scenario_module_paths_include_pyface1_python_runtime_dependencies() {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = project_root.join("services/ws-server");
    let paths = scenario_module_paths(&ws_server_dir, &["pyface1".to_string()]).unwrap();

    assert!(paths.contains(&"../ws-modules/pyface1".to_string()));
    assert!(paths.contains(&"../../data/model-modules/model-face1".to_string()));
    assert!(paths.contains(&"$(mise where npm:onnxruntime-web)/lib/node_modules/onnxruntime-web".to_string()));
    assert!(paths.contains(&"$(mise where npm:pyodide)/lib/node_modules/pyodide".to_string()));
}

#[test]
fn published_mode_uses_configured_edge_toolkit_path_for_mise_deployment() {
    let test_root = tempdir().unwrap();
    let release_root = test_root.path().join("release");
    let input_dir = test_root.path().join("input");
    let output_dir = test_root.path().join("output");
    fs::create_dir_all(release_root.join("services/ws-server/static/pkg")).unwrap();
    fs::create_dir_all(release_root.join("services/ws-wasm-agent/pkg")).unwrap();
    fs::create_dir_all(release_root.join("services/ws-modules/face-detection/pkg")).unwrap();
    fs::create_dir_all(release_root.join("data/model-modules/model-face1/pkg")).unwrap();
    fs::create_dir_all(release_root.join("config")).unwrap();
    fs::create_dir_all(&input_dir).unwrap();
    fs::write(
        release_root.join("services/ws-server/static/pkg/package.json"),
        r#"{"name":"et-ws-server-static"}"#,
    )
    .unwrap();
    fs::write(
        release_root.join("services/ws-modules/face-detection/pkg/package.json"),
        r#"{"name":"et-ws-face-detection","dependencies":{"et-model-face1":"*","onnxruntime-web":"*"}}"#,
    )
    .unwrap();
    fs::write(
        release_root.join("data/model-modules/model-face1/pkg/package.json"),
        r#"{"name":"et-model-face1"}"#,
    )
    .unwrap();
    fs::write(release_root.join("config/o2.env"), "").unwrap();

    let input_file = input_dir.join("cluster.yaml");
    fs::write(
        &input_file,
        r#"cluster_name: "published-cluster"
agents:
  - name: "camera"
    resources:
      - type: "face-detection"
"#,
    )
    .unwrap();

    generate_deployment_with_options(
        &input_file,
        &output_dir,
        None,
        &DeploymentOptions {
            mode: DeploymentMode::Published,
            edge_toolkit_path: Some(release_root.clone()),
        },
    )
    .unwrap();

    let mise = fs::read_to_string(output_dir.join("mise.toml")).unwrap();
    assert!(mise.contains(&release_root.join("target/release/et-ws-server").display().to_string()));
    assert!(!mise.contains("cargo run"));
    assert!(mise.contains(&release_root.join("services/ws-server/static").display().to_string()));
    assert!(
        mise.contains(
            &release_root
                .join("services/ws-modules/face-detection")
                .display()
                .to_string()
        )
    );
    assert!(
        mise.contains(
            &release_root
                .join("data/model-modules/model-face1")
                .display()
                .to_string()
        )
    );
    assert!(mise.contains(&release_root.join("node_modules/onnxruntime-web").display().to_string()));
    assert!(mise.contains(&release_root.join("config/o2.env").display().to_string()));
}

#[test]
fn published_mode_uses_normal_release_binary_when_edge_toolkit_path_is_source_checkout() {
    let test_root = tempdir().unwrap();
    let source_root = test_root.path().join("source");
    let input_dir = test_root.path().join("input");
    let output_dir = test_root.path().join("output");
    fs::create_dir_all(source_root.join("services/ws-server/static/pkg")).unwrap();
    fs::create_dir_all(source_root.join("services/ws-wasm-agent/pkg")).unwrap();
    fs::create_dir_all(source_root.join("services/ws-modules/face-detection/pkg")).unwrap();
    fs::create_dir_all(source_root.join("data/model-modules/model-face1/pkg")).unwrap();
    fs::create_dir_all(source_root.join("config")).unwrap();
    fs::create_dir_all(&input_dir).unwrap();
    fs::write(
        source_root.join("services/ws-modules/face-detection/pkg/package.json"),
        r#"{"name":"et-ws-face-detection","dependencies":{"et-model-face1":"*"}}"#,
    )
    .unwrap();
    fs::write(
        source_root.join("data/model-modules/model-face1/pkg/package.json"),
        r#"{"name":"et-model-face1"}"#,
    )
    .unwrap();
    fs::write(source_root.join("config/o2.env"), "").unwrap();

    let input_file = input_dir.join("cluster.yaml");
    fs::write(
        &input_file,
        r#"cluster_name: "published-cluster"
agents:
  - name: "camera"
    resources:
      - type: "face-detection"
"#,
    )
    .unwrap();

    generate_deployment_with_options(
        &input_file,
        &output_dir,
        None,
        &DeploymentOptions {
            mode: DeploymentMode::Published,
            edge_toolkit_path: Some(source_root.clone()),
        },
    )
    .unwrap();

    let mise = fs::read_to_string(output_dir.join("mise.toml")).unwrap();
    assert!(mise.contains(&source_root.join("target/release/et-ws-server").display().to_string()));
}

#[test]
fn published_mode_mounts_configured_edge_toolkit_path_for_compose_deployment() {
    let test_root = tempdir().unwrap();
    let release_root = test_root.path().join("release");
    let input_dir = test_root.path().join("input");
    let output_dir = test_root.path().join("output");
    fs::create_dir_all(release_root.join("services/ws-server/static/pkg")).unwrap();
    fs::create_dir_all(release_root.join("services/ws-wasm-agent/pkg")).unwrap();
    fs::create_dir_all(release_root.join("services/ws-modules/face-detection/pkg")).unwrap();
    fs::create_dir_all(release_root.join("data/model-modules/model-face1/pkg")).unwrap();
    fs::create_dir_all(release_root.join("config")).unwrap();
    fs::create_dir_all(&input_dir).unwrap();
    fs::write(
        release_root.join("services/ws-modules/face-detection/pkg/package.json"),
        r#"{"name":"et-ws-face-detection","dependencies":{"et-model-face1":"*","onnxruntime-web":"*"}}"#,
    )
    .unwrap();
    fs::write(
        release_root.join("data/model-modules/model-face1/pkg/package.json"),
        r#"{"name":"et-model-face1"}"#,
    )
    .unwrap();
    fs::write(release_root.join("config/o2.env"), "").unwrap();

    let input_file = input_dir.join("cluster.yaml");
    fs::write(
        &input_file,
        r#"cluster_name: "published-cluster"
agents:
  - name: "camera"
    resources:
      - type: "face-detection"
"#,
    )
    .unwrap();

    generate_deployment_with_options(
        &input_file,
        &output_dir,
        Some(OutputType::DockerCompose),
        &DeploymentOptions {
            mode: DeploymentMode::Published,
            edge_toolkit_path: Some(release_root.clone()),
        },
    )
    .unwrap();

    let compose = fs::read_to_string(output_dir.join("compose.yaml")).unwrap();
    assert!(compose.contains("image: ubuntu:24.04"));
    assert!(compose.contains("command: /usr/local/bin/et-ws-server"));
    assert!(!compose.contains("image: et-ws-server:local"));
    assert!(compose.contains(&format!(
        "- {}:/usr/local/bin/et-ws-server:ro",
        release_root.join("target/release/et-ws-server").display()
    )));
    assert!(compose.contains(&format!(
        "- {}:/app/services/ws-modules:ro",
        release_root.join("services/ws-modules").display()
    )));
    assert!(compose.contains(&format!(
        "- {}:/app/data/model-modules:ro",
        release_root.join("data/model-modules").display()
    )));
    assert!(compose.contains(&format!(
        "- {}:/app/node_modules:ro",
        release_root.join("node_modules").display()
    )));
    assert!(compose.contains(&release_root.join("config/o2.env").display().to_string()));
    assert!(compose.contains("/app/services/ws-modules/face-detection"));
    assert!(compose.contains("/app/data/model-modules/model-face1"));
}

#[test]
fn module_package_json_reads_pyproject_ws_module_dependencies() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path().join("python-module");
    fs::create_dir_all(&module_dir).unwrap();
    fs::write(
        module_dir.join("pyproject.toml"),
        r#"[project]
name = "et-ws-python-module"

[tool.ws-module.dependencies]
et-model-face1 = "*"
onnxruntime-web = "*"
"#,
    )
    .unwrap();

    let package = module_package_json(&module_dir).unwrap();

    assert_eq!(package.name.as_deref(), Some("et-ws-python-module"));
    assert_eq!(
        package.dependencies.get("et-model-face1").map(String::as_str),
        Some("*")
    );
    assert_eq!(
        package.dependencies.get("onnxruntime-web").map(String::as_str),
        Some("*")
    );
}

#[test]
fn module_package_json_reads_cargo_ws_module_dependencies() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path().join("rust-module");
    fs::create_dir_all(&module_dir).unwrap();
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

    let package = module_package_json(&module_dir).unwrap();

    assert_eq!(package.name.as_deref(), Some("et-ws-rust-module"));
    assert_eq!(
        package.dependencies.get("et-model-har-motion1").map(String::as_str),
        Some("*")
    );
}

#[test]
fn regenerate_verification_generates_all_deployment_types() {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let input_dir = verification_root.join("local/input");
    let output_dir = verification_root.join("local/output/cluster");
    fs::create_dir_all(&input_dir).unwrap();

    let input_file = input_dir.join("cluster.yaml");

    fs::write(
        &input_file,
        r#"cluster_name: "manifest-cluster"
deployment_type: "mise"
agents:
  - name: "camera"
    resources:
      - type: "face-detection"
"#,
    )
    .unwrap();

    let regenerated = regenerate_verification(&verification_root, None).unwrap();

    assert_eq!(regenerated.len(), 1);
    assert_eq!(regenerated[0].input_file, input_file);
    assert_eq!(regenerated[0].output_dir, output_dir);
    assert_eq!(regenerated[0].summary.cluster_name, "manifest-cluster");
    assert!(output_dir.join("mise.toml").exists());
    assert!(output_dir.join("compose.yaml").exists());
    assert!(output_dir.join("README.md").exists());
    let mise = fs::read_to_string(output_dir.join("mise.toml")).unwrap();
    assert!(mise.contains("export MODULES_PATHS="));
    let readme = fs::read_to_string(output_dir.join("README.md")).unwrap();
    assert!(readme.contains("`mise.toml`"));
    assert!(readme.contains("`compose.yaml`"));
    assert!(readme.contains("mise run generated-scenario"));
    assert!(readme.contains("docker compose up"));
}

#[test]
fn regenerate_verification_default_local_mode_only_scans_local_subfolder() {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let local_input_dir = verification_root.join("local/input");
    let ci_input_dir = verification_root.join("ci/input");
    let local_output_dir = verification_root.join("local/output/local-scenario");
    let ci_output_dir = verification_root.join("ci/output/ci-scenario");
    fs::create_dir_all(&local_input_dir).unwrap();
    fs::create_dir_all(&ci_input_dir).unwrap();

    let local_input = local_input_dir.join("local-scenario.yaml");
    let ci_input = ci_input_dir.join("ci-scenario.yaml");

    fs::write(
        &local_input,
        r#"cluster_name: "local-cluster"
deployment_type: "mise"
agents: []
"#,
    )
    .unwrap();
    fs::write(
        &ci_input,
        r#"cluster_name: "ci-cluster"
deployment_type: "mise"
agents: []
"#,
    )
    .unwrap();

    let regenerated = regenerate_verification(&verification_root, None).unwrap();

    assert_eq!(regenerated.len(), 1);
    assert_eq!(regenerated[0].input_file, local_input);
    assert_eq!(regenerated[0].output_dir, local_output_dir);
    assert!(local_output_dir.join("mise.toml").exists());
    assert!(local_output_dir.join("compose.yaml").exists());
    assert!(!ci_output_dir.join("mise.toml").exists());
    assert!(!ci_output_dir.join("compose.yaml").exists());
}

#[test]
fn regenerate_verification_published_mode_only_scans_published_subfolder() {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let local_input_dir = verification_root.join("local/input");
    let published_input_dir = verification_root.join("published/input");
    let local_output_dir = verification_root.join("local/output/local-scenario");
    let published_output_dir = verification_root.join("published/output/published-scenario");
    let release_root = test_root.path().join("release");
    fs::create_dir_all(&local_input_dir).unwrap();
    fs::create_dir_all(&published_input_dir).unwrap();
    fs::create_dir_all(release_root.join("services/ws-server/static")).unwrap();
    fs::create_dir_all(release_root.join("services/ws-modules")).unwrap();
    fs::create_dir_all(release_root.join("data/model-modules")).unwrap();
    fs::create_dir_all(release_root.join("target/release")).unwrap();
    fs::write(release_root.join("target/release/et-cli"), "").unwrap();
    fs::write(release_root.join("target/release/et-ws-server"), "").unwrap();

    fs::write(
        local_input_dir.join("local-scenario.yaml"),
        r#"cluster_name: "local-cluster"
deployment_mode: "local"
agents: []
"#,
    )
    .unwrap();
    fs::write(
        published_input_dir.join("published-scenario.yaml"),
        r#"cluster_name: "published-cluster"
deployment_mode: "published"
agents: []
"#,
    )
    .unwrap();

    regenerate_verification_with_options(
        &verification_root,
        Some(OutputType::Mise),
        &DeploymentOptions {
            mode: DeploymentMode::Published,
            edge_toolkit_path: Some(release_root.clone()),
        },
    )
    .unwrap();

    let published_mise = fs::read_to_string(published_output_dir.join("mise.toml")).unwrap();
    assert!(!local_output_dir.join("mise.toml").exists());
    assert!(published_mise.contains(&release_root.join("target/release/et-ws-server").display().to_string()));
    assert!(!published_mise.contains("cargo run"));
}

#[test]
fn regenerate_verification_published_mode_requires_edge_toolkit_path() {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let published_input_dir = verification_root.join("published/input");
    fs::create_dir_all(&published_input_dir).unwrap();
    fs::write(
        published_input_dir.join("published-scenario.yaml"),
        r#"cluster_name: "published-cluster"
deployment_mode: "published"
agents: []
"#,
    )
    .unwrap();

    let error = regenerate_verification_with_options(
        &verification_root,
        Some(OutputType::Mise),
        &DeploymentOptions {
            mode: DeploymentMode::Published,
            edge_toolkit_path: None,
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("requires --edge-toolkit-path"));
}
