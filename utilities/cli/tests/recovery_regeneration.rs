//! Recovery regeneration replaces unsupported launchers without removing unrelated user files.
#![cfg(test)]

use std::path::{Path, PathBuf};

use et_cli::{OutputType, regenerate_verification};
use fs_err as fs;
use serde_json::{Value, json};

/// Decode the reference scenario as a mutable document for regeneration transitions.
fn scenario() -> Value {
    let root = edge_toolkit::config::get_project_root();
    let document = fs::read_to_string(root.join("verification/local/input/pyo3-math1-recovery.yaml")).unwrap();
    serde_yaml::from_str(&document).unwrap()
}

/// Write a scenario document into the verification input tree.
fn write_input(root: &Path, input: &Value) -> PathBuf {
    let path = root.join("scenario.yaml");
    let document = serde_yaml::to_string(input).unwrap();
    fs::write(&path, document).unwrap();
    path
}

/// Generate only supported verification formats for recovery-enabled scenarios.
#[test]
fn verification_generation_limits_recovery_scenarios_to_supported_formats() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("local/input");
    fs::create_dir_all(&input).unwrap();
    let _file = write_input(&input, &scenario());
    let generated = regenerate_verification(root.path(), None).unwrap();
    assert_eq!(generated.len(), 1);
    let output = &generated[0].output_dir;
    assert!(output.join("k3s.yaml").is_file());
    assert!(!output.join("mise.toml").exists());
    let readme = fs::read_to_string(output.join("README.md")).unwrap();
    assert!(readme.contains("services/recovery-supervisor/Dockerfile"));
}

/// Every format a scenario with runners renders: all but the board's, which runs none.
fn runner_formats() -> impl Iterator<Item = &'static OutputType> {
    OutputType::ALL
        .iter()
        .filter(|kind| **kind != OutputType::FirmwarePico2)
}

/// Start with actual outputs from every generator, rather than fabricated files that may not match their layout.
fn verification_without_recovery() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let input_dir = root.path().join("local/input");
    fs::create_dir_all(&input_dir).unwrap();
    let mut input = scenario();
    input["recovery"] = Value::Null;
    let _file = write_input(&input_dir, &input);
    let generated = regenerate_verification(root.path(), None).unwrap();
    let output = generated.first().unwrap().output_dir.clone();
    for kind in runner_formats() {
        assert!(output.join(kind.output_file_name()).is_file());
    }
    (root, output)
}

/// Snapshot the launcher files that a recovery-free K3s-only regeneration must leave untouched.
fn non_k3s_snapshot(output: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    runner_formats()
        .filter(|kind| **kind != OutputType::K3s)
        .map(|kind| {
            let path = output.join(kind.output_file_name());
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

/// Enabling recovery removes obsolete launchers but preserves shared image files and unrelated user content.
#[test]
fn recovery_regeneration_removes_only_stale_non_k3s_launchers() {
    let (root, output) = verification_without_recovery();
    fs::write(output.join("notes.txt"), b"keep notes").unwrap();
    let custom = output.join("custom");
    fs::create_dir(&custom).unwrap();
    fs::write(custom.join("compose.yaml"), b"keep custom deployment").unwrap();
    let _file = write_input(&root.path().join("local/input"), &scenario());
    let regenerated = regenerate_verification(root.path(), None).unwrap();
    assert_eq!(regenerated.len(), 1);
    for kind in OutputType::ALL {
        assert_eq!(output.join(kind.output_file_name()).is_file(), *kind == OutputType::K3s);
    }
    for name in [
        "Dockerfile",
        "Dockerfile.dockerignore",
        "README.md",
        "secrets.env",
        ".gitignore",
    ] {
        assert!(output.join(name).is_file(), "missing shared artifact {name}");
    }
    assert_eq!(fs::read(output.join("notes.txt")).unwrap(), b"keep notes");
    assert_eq!(
        fs::read(custom.join("compose.yaml")).unwrap(),
        b"keep custom deployment"
    );
    let text = fs::read_to_string(output.join("k3s.yaml")).unwrap();
    assert!(text.contains("recovery-supervisor"));
    let _regenerated = regenerate_verification(root.path(), Some(OutputType::K3s)).unwrap();
    assert!(output.join("k3s.yaml").is_file());
}

/// Selecting K3s alone does not remove the other generated formats when recovery remains disabled.
#[test]
fn regeneration_preserves_other_formats_without_recovery() {
    let (root, output) = verification_without_recovery();
    let before = non_k3s_snapshot(&output);
    let _regenerated = regenerate_verification(root.path(), Some(OutputType::K3s)).unwrap();
    assert_eq!(non_k3s_snapshot(&output), before);
}

/// Invalid recovery settings must fail validation before existing output files can be removed or overwritten.
#[test]
fn invalid_recovery_does_not_remove_existing_launchers() {
    let (root, output) = verification_without_recovery();
    let before = non_k3s_snapshot(&output);
    let mut input = scenario();
    input["recovery"]["job_id"] = json!("");
    let _file = write_input(&root.path().join("local/input"), &input);
    let _error = regenerate_verification(root.path(), None).unwrap_err();
    assert_eq!(non_k3s_snapshot(&output), before);
}

/// A blocked obsolete path is an error, not permission to remove a directory or silently retain a bypass launcher.
#[test]
fn recovery_cleanup_reports_blocked_paths_without_removing_directory_contents() {
    let (root, output) = verification_without_recovery();
    let blocked = output.join("mise.toml");
    fs::remove_file(&blocked).unwrap();
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("notes.txt"), b"keep nested data").unwrap();
    let _file = write_input(&root.path().join("local/input"), &scenario());
    let error = regenerate_verification(root.path(), None).unwrap_err();
    assert!(matches!(error, et_cli::CliError::Io(_)));
    assert!(error.to_string().contains("mise.toml"));
    assert_eq!(fs::read(blocked.join("notes.txt")).unwrap(), b"keep nested data");
}
