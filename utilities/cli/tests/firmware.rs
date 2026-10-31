//! Covers the firmware formats through `firmware-pico2`: which modules a firmware build embeds and which it
//! redirects, and the scenarios it refuses because a board runs one hub and nothing else.
#![cfg(test)]

use std::path::PathBuf;

use et_cli::{OutputType, generate_deployment, regenerate_verification};
use fs_err as fs;
use tempfile::{TempDir, tempdir};

/// A browser agent running math1, which the page loads from the board.
const BROWSER: &str = r#"cluster_name: "board"
agents:
  - name: "math1-twin"
    resources:
      - type: "math1"
"#;

/// The file `firmware-pico2` writes.
const CONFIG_FILE: &str = "mise.firmware-pico2.toml";

/// The generated firmware config, parsed.
#[derive(Debug, serde::Deserialize)]
struct FirmwareConfig {
    tasks: Tasks,
}

#[derive(Debug, serde::Deserialize)]
struct Tasks {
    #[serde(rename = "firmware-build")]
    build: Task,
    #[serde(rename = "firmware-flash")]
    flash: Task,
    #[serde(rename = "firmware-host-test")]
    host_test: Task,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Task {
    depends: Vec<String>,
    dir: String,
    env: std::collections::BTreeMap<String, String>,
    run: String,
}

/// What generating `input` in `test_root` as `firmware-pico2` returns, the error's text or empty, and its directory.
fn generate_in(test_root: TempDir, input: &str) -> (TempDir, PathBuf, String) {
    let input_file = test_root.path().join("board.yaml");
    fs::write(&input_file, input).unwrap();
    let output_dir = test_root.path().join("output");
    let outcome = generate_deployment(&input_file, &output_dir, Some(OutputType::FirmwarePico2))
        .map_or_else(|error| error.to_string(), |_summary| String::default());
    (test_root, output_dir, outcome)
}

/// The deployment `input` generates, which has to succeed.
fn generated(input: &str) -> (TempDir, FirmwareConfig, String) {
    generated_in(tempdir().unwrap(), input)
}

/// The deployment `input`, written to a scenario in `test_root`, generates, which has to succeed.
fn generated_in(test_root: TempDir, input: &str) -> (TempDir, FirmwareConfig, String) {
    let (test_root, output_dir, outcome) = generate_in(test_root, input);
    assert_eq!(outcome, "");
    assert_eq!(OutputType::FirmwarePico2.output_file_name(), CONFIG_FILE);
    let config = toml::from_str(&fs::read_to_string(output_dir.join(CONFIG_FILE)).unwrap()).unwrap();
    let readme = fs::read_to_string(output_dir.join("README.md")).unwrap();
    (test_root, config, readme)
}

#[test]
fn it_embeds_the_repository_modules_and_redirects_the_npm_packages() {
    let (_root, config, readme) = generated(BROWSER);
    let build = &config.tasks.build;
    for module in [
        "services/ws-modules/math1",
        "services/ws-server/static",
        "services/ws-wasm-agent",
    ] {
        assert!(build.run.contains(&format!("$PWD/{module}")));
    }
    assert!(!build.run.contains("onnxruntime-web"));
    assert!(build.run.contains("mise -E rust run build-firmware-pico2w"));
    assert_eq!(build.env["MODULES_ROOT"], "@edge-toolkit/et-ws-server-static");
    assert_eq!(build.env["MODULES_ELSEWHERE"], "onnxruntime-web, stats-gl");
    assert_eq!(build.env["MODULES_ELSEWHERE_URL"], "https://cdn.jsdelivr.net/npm");

    assert_eq!(config.tasks.flash.depends, ["firmware-build"]);
    assert_eq!(config.tasks.flash.dir, build.dir);
    assert!(config.tasks.flash.run.starts_with("picotool load"));
    assert!(readme.contains("## Run As Firmware On A Raspberry Pi Pico 2 W"));
    assert!(readme.contains("MISE_ENV=firmware-pico2 mise run firmware-flash"));
}

#[test]
fn its_host_test_serves_the_modules_the_build_embeds() {
    let (_root, config, readme) = generated(BROWSER);
    let (build, host_test) = (&config.tasks.build, &config.tasks.host_test);
    assert_eq!(host_test.env, build.env);
    assert_eq!(host_test.dir, build.dir);
    let (paths, _) = build.run.split_once("mise -E rust").unwrap();
    let (host_paths, command) = host_test.run.split_once("mise run").unwrap();
    assert_eq!(host_paths, paths);
    assert_eq!(command, " cargo-test-pkg et-int-ws-server-pico2w\n");
    assert!(readme.contains("MISE_ENV=firmware-pico2 mise run firmware-host-test"));
}

#[test]
fn it_embeds_a_module_the_scenario_keeps_beside_it() {
    let test_root = tempdir().unwrap();
    let module_dir = test_root.path().join("mine");
    fs::create_dir_all(&module_dir).unwrap();
    fs::write(
        module_dir.join("package.json"),
        r#"{"name": "mine", "main": "mine.js"}"#,
    )
    .unwrap();
    // Named relative to the scenario, as `module_paths:` is meant to be; the temporary directory holding both, which
    // on Windows runs through `RUNNER~1`, is not the scenario author's to name.
    let input = format!("{BROWSER}module_paths: [\"mine\"]\n").replace(r#"type: "math1""#, r#"type: "mine""#);

    let module_path = module_dir.display().to_string();
    let (_root, config, _readme) = generated_in(test_root, &input);
    assert!(config.tasks.build.run.contains(&module_path));
}

#[test]
fn it_refuses_what_the_board_cannot_run() {
    let runner = BROWSER.replace("    resources:", "    runner: \"web\"\n    resources:");
    let machines = format!("{BROWSER}machines:\n  - name: \"server\"\n    address: \"10.0.0.1\"\n");
    let published = format!("{BROWSER}artifact_source: \"published\"\n");
    for (input, reason) in [
        (runner, "starts a runner"),
        (machines, "spans several machines"),
        (published, "uses published artifacts"),
    ] {
        let (_root, output_dir, outcome) = generate_in(tempdir().unwrap(), &input);
        assert!(outcome.starts_with("The Raspberry Pi Pico 2 W firmware runs one hub and nothing else"));
        assert!(outcome.contains(reason));
        assert!(!output_dir.join(CONFIG_FILE).exists());
    }
}

#[test]
fn verification_renders_it_only_for_a_scenario_the_board_can_run() {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let input_dir = verification_root.join("local/input");
    fs::create_dir_all(&input_dir).unwrap();
    fs::write(input_dir.join("browser.yaml"), BROWSER).unwrap();
    let headless = BROWSER.replace("    resources:", "    runner: \"web\"\n    resources:");
    fs::write(input_dir.join("headless.yaml"), headless).unwrap();

    let _regenerated = regenerate_verification(&verification_root, None).unwrap();
    let output = verification_root.join("local/output");
    assert!(output.join("browser").join(CONFIG_FILE).is_file());
    assert!(output.join("browser/mise.toml").is_file());
    assert!(!output.join("headless").join(CONFIG_FILE).exists());
    assert!(output.join("headless/mise.toml").is_file());
}
