use std::fs;

use tempfile::tempdir;

use crate::{docker_image_module_paths, generate_deployment, regenerate_verification, scenario_module_paths};

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
    let paths = docker_image_module_paths(&["face-detection".to_string()]);

    assert_eq!(paths[0], "/app/services/ws-server/static");
    assert!(paths.contains(&"/app/services/ws-wasm-agent".to_string()));
    assert!(paths.contains(&"/app/data/model-modules".to_string()));
    assert!(paths.contains(&"/app/node_modules/onnxruntime-web".to_string()));
    assert!(paths.contains(&"/app/node_modules/pyodide".to_string()));
    assert!(paths.contains(&"/app/services/ws-modules/face-detection".to_string()));
}

#[test]
fn scenario_module_paths_include_only_selected_workflow_modules() {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = project_root.join("services/ws-server");
    let paths = scenario_module_paths(&ws_server_dir, &["face-detection".to_string(), "har1".to_string()]);

    assert_eq!(
        paths,
        vec![
            "static".to_string(),
            "../ws-wasm-agent".to_string(),
            "../../data/model-modules".to_string(),
            "../ws-modules/face-detection".to_string(),
            "../ws-modules/har1".to_string(),
        ],
    );
    assert!(!paths.contains(&"../ws-modules".to_string()));
    assert!(!paths.contains(&"../ws-modules/data1".to_string()));
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
fn regenerate_verification_scans_multiple_verification_subfolders() {
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

    assert_eq!(regenerated.len(), 2);
    assert_eq!(regenerated[0].input_file, ci_input);
    assert_eq!(regenerated[0].output_dir, ci_output_dir);
    assert_eq!(regenerated[1].input_file, local_input);
    assert_eq!(regenerated[1].output_dir, local_output_dir);
    assert!(local_output_dir.join("mise.toml").exists());
    assert!(local_output_dir.join("compose.yaml").exists());
    assert!(ci_output_dir.join("mise.toml").exists());
    assert!(ci_output_dir.join("compose.yaml").exists());
}
