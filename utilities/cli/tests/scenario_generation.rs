#![cfg(test)]

use et_cli::{
    docker_image_module_paths, generate_deployment, hub_service_ws_url, hub_ws_url, module_package_json,
    regenerate_verification, scenario_module_paths,
};
use fs_err as fs;
use serde::Deserialize as _;
use tempfile::tempdir;

/// Lay out a `verification/` tree holding one scenario input, and return its root and the output directory.
///
/// Both regeneration tests below need the same four paths in the same shape and differ only in the document
/// they write, so the scaffolding lives here once. The temp root is returned alongside them because dropping
/// it deletes the tree, so the caller has to keep it alive for the length of the test.
fn scenario_tree(input: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let input_dir = verification_root.join("local/input");
    let output_dir = verification_root.join("local/output/cluster");
    fs::create_dir_all(&input_dir).unwrap();
    fs::write(input_dir.join("cluster.yaml"), input).unwrap();
    (test_root, verification_root, output_dir)
}

/// Write `input` as a scenario input file and return the error that generating from it produces.
///
/// Every rejection test below needs the same scaffolding -- a temp root, an input and output directory, the YAML
/// written out -- and differs only in the document and the message it expects, so the scaffolding lives here once.
fn deployment_error_for(input: &str) -> String {
    let test_root = tempdir().unwrap();
    let input_dir = test_root.path().join("input");
    let output_dir = test_root.path().join("output");
    fs::create_dir_all(&input_dir).unwrap();

    let input_file = input_dir.join("cluster.yaml");
    fs::write(&input_file, input).unwrap();

    generate_deployment(&input_file, &output_dir, None)
        .unwrap_err()
        .to_string()
}

#[test]
fn generate_deployment_rejects_unsupported_deployment_type() {
    let error = deployment_error_for(
        r#"cluster_name: "test-cluster"
deployment_type: yaml
agents: []
"#,
    );

    assert!(error.contains("Unsupported deployment_type"), "got: {error}");
}

#[test]
fn generate_deployment_rejects_a_cluster_name_that_is_not_an_rfc_1123_label() {
    // The name reaches a shell in the generated README (`scenario=<name>`) and a Kubernetes namespace in
    // `k3s.yaml`, so anything outside the label alphabet is either an injection vector or a manifest the API
    // server rejects. A `;` is the shell half of that in its shortest form.
    let error = deployment_error_for(
        r#"cluster_name: "oops; echo pwned"
deployment_type: "mise"
agents: []
"#,
    );

    assert!(
        error.contains("RFC 1123") && error.contains(';'),
        "expected an invalid-name error naming the character, got: {error}"
    );
}

#[test]
fn generate_deployment_rejects_a_runner_named_after_a_generated_task() {
    // `ws-server` is the hub's own task and compose service. Left unchecked the runner would replace it, and the
    // deployment would come up with nothing to connect to rather than reporting a problem.
    let error = deployment_error_for(
        r#"cluster_name: "name-collision"
deployment_type: "mise"
agents:
  - name: "ws-server"
    runner: "web"
    resources:
      - type: "math1"
"#,
    );

    assert!(
        error.contains("ws-server") && error.contains("already uses"),
        "expected a reserved-name error, got: {error}"
    );
}

#[test]
fn generate_deployment_rejects_two_runners_with_the_same_name() {
    // Two agents sharing a name derive one runner name; mise would keep the last task inserted under it and
    // compose would emit a duplicate service key, so one of the two runners would silently vanish.
    let error = deployment_error_for(
        r#"cluster_name: "name-collision"
deployment_type: "mise"
agents:
  - name: "twin"
    runner: "web"
    resources:
      - type: "math1"
  - name: "twin"
    runner: "web"
    resources:
      - type: "math1-sender"
"#,
    );

    assert!(
        error.contains("twin") && error.contains("more than once"),
        "expected a duplicate-name error, got: {error}"
    );
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
            "$(cargo run --quiet -p et-cli -- npm-module-path --package onnxruntime-web)".to_string(),
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
    assert!(paths.contains(&"$(cargo run --quiet -p et-cli -- npm-module-path --package onnxruntime-web)".to_string()));
    // pyface1 calls `micropip.install`, so it needs the full GitHub-release distribution rather than the npm one.
    assert!(paths.contains(&"$(mise where http:pyodide)".to_string()));
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
    let (_test_root, verification_root, output_dir) = scenario_tree(
        r#"cluster_name: "manifest-cluster"
deployment_type: "mise"
agents:
  - name: "camera"
    resources:
      - type: "face-detection"
"#,
    );
    let input_file = verification_root.join("local/input/cluster.yaml");

    let regenerated = regenerate_verification(&verification_root, None).unwrap();

    assert_eq!(regenerated.len(), 1);
    assert_eq!(regenerated[0].input_file, input_file);
    assert_eq!(regenerated[0].output_dir, output_dir);
    assert_eq!(regenerated[0].summary.cluster_name, "manifest-cluster");
    assert!(output_dir.join("mise.toml").exists());
    assert!(output_dir.join("compose.yaml").exists());
    assert!(output_dir.join("k3s.yaml").exists());
    assert!(output_dir.join("README.md").exists());
    let mise = fs::read_to_string(output_dir.join("mise.toml")).unwrap();
    assert!(mise.contains("MODULES_PATHS=\""));
    assert!(mise.contains("export MODULES_PATHS\n"));
    // The path list is assembled by appending to the variable, never by continuing the line with a trailing `\`.
    assert!(
        !mise.contains('\\'),
        "generated mise.toml must carry no line-continuations"
    );
    let readme = fs::read_to_string(output_dir.join("README.md")).unwrap();
    assert!(readme.contains("`mise.toml`"));
    assert!(readme.contains("`compose.yaml`"));
    assert!(readme.contains("mise run generated-scenario"));
    assert!(readme.contains("docker compose up"));
    assert!(readme.contains("kubectl apply -f k3s.yaml"));
}

/// The generated manifests describe the whole scenario and keep the credential out of the file.
///
/// The credential half is the point of the `envFrom` assertion: the value lives in the uncommitted env file
/// and reaches the pods through a `Secret` the operator creates, so a regression that inlined it would put a
/// password into a committed tree. Asserting the literal is absent is what catches that.
/// Regenerate one k3s scenario and return its output directory.
///
/// The two tests below assert on different halves of the same manifest, so the generation lives here once. The
/// temp root comes back alongside the directory because dropping it deletes the tree.
fn k3s_scenario() -> (tempfile::TempDir, std::path::PathBuf) {
    let (test_root, verification_root, output_dir) = scenario_tree(
        r#"cluster_name: "k3s-cluster"
deployment_type: "k3s"
agents:
  - name: "math1-twin"
    runner: "wasi"
    resources:
      - type: "wasi-math1"
"#,
    );

    let _regenerated = regenerate_verification(&verification_root, None).unwrap();
    (test_root, output_dir)
}

#[test]
fn regenerate_verification_emits_one_k3s_document_per_component() {
    let (_test_root, output_dir) = k3s_scenario();

    let text = fs::read_to_string(output_dir.join("k3s.yaml")).unwrap();
    let documents: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(&text)
        .map(|document| serde_yaml::Value::deserialize(document).unwrap())
        .collect();
    let kinds: Vec<&str> = documents
        .iter()
        .map(|document| document.get("kind").and_then(serde_yaml::Value::as_str).unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "Namespace",
            "ConfigMap",
            "PersistentVolumeClaim",
            "Deployment",
            "Service",
            "PersistentVolumeClaim",
            "Deployment",
            "Service",
            "Deployment",
        ],
        "one document per component, runners last"
    );

    // Every namespaced object names the scenario's namespace, so `kubectl apply` needs no `-n`.
    for document in documents.iter().skip(1) {
        let namespace = document.get("metadata").and_then(|meta| meta.get("namespace"));
        assert_eq!(
            namespace.and_then(serde_yaml::Value::as_str),
            Some("et-k3s-cluster"),
            "every object after the Namespace carries it: {document:?}"
        );
    }
}

#[test]
fn regenerate_verification_keeps_the_credential_out_of_the_k3s_manifest() {
    let (_test_root, output_dir) = k3s_scenario();

    let text = fs::read_to_string(output_dir.join("k3s.yaml")).unwrap();
    let secrets = fs::read_to_string(output_dir.join("secrets.env")).unwrap();
    let password = secrets
        .lines()
        .find_map(|line| line.strip_prefix("ZO_ROOT_USER_PASSWORD="))
        .unwrap();
    assert!(
        !text.contains(password),
        "the scenario credential must never reach a committed manifest"
    );
    assert!(
        text.contains("et-k3s-cluster-secrets"),
        "the Secret is referenced by name"
    );

    // The runners address the hub by its Service, not by the `localhost` the host-networked formats use.
    // Asserted against the builders rather than two literals, so a port change cannot leave the test passing
    // against a URL the generator no longer emits.
    assert!(text.contains(&hub_service_ws_url()));
    assert!(!text.contains(&hub_ws_url()));
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
