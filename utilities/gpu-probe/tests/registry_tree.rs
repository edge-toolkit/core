//! Covers a registry directory: filing a submission under its interface's file, the registry read as one document,
//! checking every file, and filing the submissions waiting in `submitted/`.
#![cfg(test)]

use std::path::Path;

use et_gpu_probe_server::registry::{Outcome, RegisterError, RegistryTree, Submission};
use tempfile::TempDir;

/// The registry's header.
const HEADER: &str = "# The header.\nprobe_set: 1\n";

/// The `webgpu-split` interface's file, holding one known signature.
const SPLIT: &str = r#"signatures:
  - interface: webgpu-split
    label: "Known GPU"
    seen:
      - browser: chrome
        os: macos
        cpu_arch: aarch64
        gpu: apple
        backend: metal
        window: normal
    results:
      fma: "33800000"
"#;

/// The `canvas` interface's file, holding one known signature.
const CANVAS: &str = r#"signatures:
  - interface: canvas
    label: "A canvas"
    seen:
      - browser: firefox
        os: linux
        cpu_arch: x86_64
        gpu: unknown
        backend: unknown
        window: normal
    results:
      hash: "0a"
"#;

/// A submitted file, as the hosted probe page writes one.
///
/// It holds a new sighting of the known signature, the same sighting again, and a signature through an interface the
/// registry has no file for yet.
const SUBMITTED: &str = r#"submissions:
  - interface: "webgpu-split"
    label: "Known GPU on Firefox"
    seen:
      browser: "firefox"
      os: "macos"
      cpu_arch: "aarch64"
      gpu: "apple"
      backend: "metal"
      window: "normal"
    results:
      fma: "33800000"
  - interface: "webgpu-split"
    label: "Known GPU on Firefox"
    seen:
      browser: "firefox"
      os: "macos"
      cpu_arch: "aarch64"
      gpu: "apple"
      backend: "metal"
      window: "normal"
    results:
      fma: "33800000"
  - interface: "webgl2-clear"
    label: "Withheld"
    seen:
      browser: "tor-browser"
      os: "linux"
      cpu_arch: "x86_64"
      gpu: "unknown"
      backend: "unknown"
      window: "private"
    blocked: true
    reason: "WebGL is disabled"
"#;

/// A registry directory holding [`HEADER`], [`SPLIT`] and [`CANVAS`], removed when the returned guard drops.
fn tree() -> (TempDir, RegistryTree) {
    let dir = tempfile::tempdir().unwrap();
    fs_err::create_dir(dir.path().join("curated")).unwrap();
    fs_err::write(dir.path().join("header.yaml"), HEADER).unwrap();
    fs_err::write(dir.path().join("curated/webgpu-split.yaml"), SPLIT).unwrap();
    fs_err::write(dir.path().join("curated/canvas.yaml"), CANVAS).unwrap();
    let registry = RegistryTree::new(dir.path().to_owned());
    (dir, registry)
}

/// Put `text` in the registry under `dir` as the submitted file `name`.
fn submit(dir: &Path, name: &str, text: &str) {
    fs_err::create_dir_all(dir.join("submitted")).unwrap();
    fs_err::write(dir.join("submitted").join(name), text).unwrap();
}

/// A new signature through `interface`, seen on Firefox.
fn new_through(interface: &str) -> Submission {
    serde_json::from_str(&format!(
        r#"{{"interface": "{interface}", "label": "New", "results": {{"a": "1"}},
        "seen": {{"browser": "firefox", "os": "linux", "cpu_arch": "x86_64", "gpu": "intel", "backend": "vulkan",
        "window": "normal"}}}}"#
    ))
    .unwrap()
}

#[test]
fn a_submission_is_filed_under_its_interface_whose_file_is_made_if_it_has_none() {
    let (dir, registry) = tree();
    assert_eq!(
        registry.register(&new_through("webgpu-clear")).unwrap(),
        Outcome::Added(1)
    );
    let made = fs_err::read_to_string(dir.path().join("curated/webgpu-clear.yaml")).unwrap();
    assert!(made.starts_with("signatures:\n  - interface: webgpu-clear\n"));
    assert_eq!(
        registry.register(&new_through("webgpu-split")).unwrap(),
        Outcome::Added(2)
    );
    assert_eq!(
        fs_err::read_to_string(dir.path().join("curated/canvas.yaml")).unwrap(),
        CANVAS
    );
    assert_eq!(fs_err::read_to_string(dir.path().join("header.yaml")).unwrap(), HEADER);
    // The lock files are kept apart from the entries.
    let mut curated: Vec<_> = fs_err::read_dir(dir.path().join("curated"))
        .unwrap()
        .map(|found| found.unwrap().file_name())
        .collect();
    curated.sort();
    assert_eq!(curated, ["canvas.yaml", "webgpu-clear.yaml", "webgpu-split.yaml"]);
    assert!(dir.path().join("writing/webgpu-clear.yaml.lock").exists());
}

#[test]
fn an_interface_that_is_not_a_name_names_no_file() {
    let (dir, registry) = tree();
    let refused = registry.register(&new_through("nested/escaped")).unwrap_err();
    assert!(matches!(refused, RegisterError::Interface));
    assert!(!dir.path().join("curated/nested").exists());
}

#[test]
fn the_registry_reads_as_one_document_of_its_header_then_each_file_in_name_order() {
    let (dir, registry) = tree();
    let expected = [
        HEADER,
        "signatures:\n",
        CANVAS.trim_start_matches("signatures:\n"),
        SPLIT.trim_start_matches("signatures:\n"),
    ]
    .concat();
    assert_eq!(registry.text().unwrap(), expected);
    // A header with no last newline is given one before the entries start.
    fs_err::write(dir.path().join("header.yaml"), HEADER.trim_end()).unwrap();
    assert_eq!(registry.text().unwrap(), expected);
}

#[test]
fn a_registry_missing_its_header_or_with_a_file_not_of_entries_does_not_read() {
    let (dir, registry) = tree();
    fs_err::write(dir.path().join("curated/canvas.yaml"), "- not: entries\n").unwrap();
    let refused = registry.text().unwrap_err();
    assert!(matches!(*refused.problem, RegisterError::NotEntries));
    assert!(refused.path.ends_with("canvas.yaml"));
    assert!(
        refused
            .to_string()
            .ends_with("canvas.yaml: a file of entries must start with a `signatures:` line")
    );

    fs_err::remove_file(dir.path().join("header.yaml")).unwrap();
    assert!(matches!(*registry.text().unwrap_err().problem, RegisterError::Io(_)));
}

#[test]
fn a_registry_with_no_curated_or_submitted_directory_has_neither_entries_nor_waiting_submissions() {
    let dir = tempfile::tempdir().unwrap();
    fs_err::write(dir.path().join("header.yaml"), HEADER).unwrap();
    let registry = RegistryTree::new(dir.path().to_owned());
    assert_eq!(registry.text().unwrap(), [HEADER, "signatures:\n"].concat());
    assert_eq!(registry.check().unwrap(), 0);
    assert_eq!(registry.fold().unwrap().len(), 0);
}

#[test]
fn a_part_of_the_registry_that_is_not_a_directory_does_not_read() {
    let (dir, registry) = tree();
    fs_err::write(dir.path().join("submitted"), "a file").unwrap();
    let refused = registry.check().unwrap_err();
    assert!(matches!(*refused.problem, RegisterError::Io(_)));
    assert!(refused.path.ends_with("submitted"));
}

#[test]
fn a_check_counts_the_waiting_submissions_and_passes_over_other_files() {
    let (dir, registry) = tree();
    submit(dir.path(), "0123456789abcdef.yaml", SUBMITTED);
    submit(dir.path(), "notes.txt", "not a submission");
    assert_eq!(registry.check().unwrap(), 3);
}

#[test]
fn a_check_finds_an_entry_filed_under_another_interface() {
    let (dir, registry) = tree();
    fs_err::write(dir.path().join("curated/webgpu.yaml"), SPLIT).unwrap();
    let refused = registry.check().unwrap_err();
    assert!(matches!(&*refused.problem, RegisterError::Misfiled(interface) if interface == "webgpu-split"));
    assert!(refused.path.ends_with("webgpu.yaml"));
}

#[test]
fn a_check_finds_a_submission_the_registry_would_refuse_or_cannot_read() {
    let (dir, registry) = tree();
    submit(dir.path(), "bad.yaml", &SUBMITTED.replace("\"Withheld\"", "\"\""));
    let refused = registry.check().unwrap_err();
    assert!(matches!(&*refused.problem, RegisterError::Text(field) if field == "label"));
    assert!(refused.path.ends_with("bad.yaml"));

    submit(dir.path(), "bad.yaml", "submissions: [\n");
    assert!(matches!(
        *registry.check().unwrap_err().problem,
        RegisterError::Parse(_)
    ));
}

#[test]
fn folding_files_each_waiting_submission_and_removes_its_file() {
    let (dir, registry) = tree();
    submit(dir.path(), "0123456789abcdef.yaml", SUBMITTED);
    let folded = registry.fold().unwrap();
    assert_eq!(folded.len(), 1);
    let (path, outcomes) = folded.first().unwrap();
    assert!(path.ends_with("0123456789abcdef.yaml"));
    assert_eq!(
        outcomes,
        &[
            Outcome::Sighted("Known GPU".to_owned()),
            Outcome::Present("Known GPU".to_owned()),
            Outcome::Added(1),
        ]
    );
    assert!(!path.exists());
    let filed = fs_err::read_to_string(dir.path().join("curated/webgpu-split.yaml")).unwrap();
    assert!(filed.contains("      - browser: firefox\n"));
    assert!(dir.path().join("curated/webgl2-clear.yaml").exists());
    assert_eq!(registry.check().unwrap(), 0);
}

#[test]
fn a_refused_submission_stops_folding_and_leaves_its_file_waiting() {
    let (dir, registry) = tree();
    let refused_last = SUBMITTED.replace("\"WebGL is disabled\"", "\"\"");
    submit(dir.path(), "0123456789abcdef.yaml", &refused_last);
    let refused = registry.fold().unwrap_err();
    assert!(matches!(&*refused.problem, RegisterError::Text(field) if field == "reason"));
    assert!(dir.path().join("submitted/0123456789abcdef.yaml").exists());
    // The sighting before it was filed all the same, and is found present when the file is folded again.
    let filed = fs_err::read_to_string(dir.path().join("curated/webgpu-split.yaml")).unwrap();
    assert!(filed.contains("      - browser: firefox\n"));
}

#[test]
fn the_registry_in_the_repository_reads_and_checks_clean() {
    let registry = RegistryTree::repository();
    let _waiting = registry.check().unwrap();
    assert!(registry.text().unwrap().contains("\nsignatures:\n  - interface: "));
}
