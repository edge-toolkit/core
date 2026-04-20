use super::*;

#[test]
fn generate_mise_deployment_writes_mise_tasks() {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let test_root = std::env::temp_dir().join(format!("et-cli-{suffix}"));
    let input_dir = test_root.join("input");
    let output_dir = test_root.join("output");
    fs::create_dir_all(&input_dir).unwrap();

    let input_file = input_dir.join("cluster.yaml");
    fs::write(
        &input_file,
        r#"cluster_name: "test-cluster"
deployment_type: "mise"
agents:
  - name: "camera"
    count: 2
    capabilities: [camera]
    modules:
      - name: "face-detection"
        config:
          inference_interval_ms: 500
  - name: "tracker"
    capabilities: [motion_sensors]
    modules:
      - name: "har1"
"#,
    )
    .unwrap();

    generate_deployment(&input_file, &output_dir, None).unwrap();

    let content = fs::read_to_string(output_dir.join(".mise.toml")).unwrap();
    assert!(!content.contains("[env]"));
    assert!(!content.contains("EDGE_CLUSTER_NAME"));
    assert!(content.contains("[tasks.ws-server]"));
    assert!(content.contains("run = \"cargo run\""));
    assert!(content.contains("OTLP_AUTH_PASSWORD = \"1234\""));
    assert!(content.contains("[tasks.build-ws-wasm-agent]"));
    assert!(content.contains(WS_WASM_AGENT_SOURCES));
    assert!(content.contains("outputs = [\"pkg/**/*\"]"));
    assert!(content.contains("RUSTUP_TOOLCHAIN = \"nightly\""));
    assert!(content.contains("[tasks.\"build-ws-face-detection-module\"]"));
    assert!(content.contains("services/ws-modules/face-detection"));
    assert!(content.contains(WS_MODULE_SOURCES));
    assert!(content.contains("[tasks.\"build-ws-har1-module\"]"));
    assert!(content.contains("services/ws-modules/har1"));
    assert!(
        content.contains(
            "depends = [\"build-ws-wasm-agent\", \"build-ws-face-detection-module\", \"build-ws-har1-module\"]"
        )
    );
    assert!(content.contains("[tasks.generated-scenario]"));
    assert!(content.contains("depends = [\"build-wasm\"]"));
    assert!(content.contains("run = \"cargo run\""));
    assert!(content.contains("[tasks.generated-scenario.env]"));

    fs::remove_dir_all(test_root).unwrap();
}

#[test]
fn generate_deployment_rejects_unsupported_deployment_type() {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let test_root = std::env::temp_dir().join(format!("et-cli-unsupported-{suffix}"));
    let input_dir = test_root.join("input");
    let output_dir = test_root.join("output");
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

    fs::remove_dir_all(test_root).unwrap();
}
