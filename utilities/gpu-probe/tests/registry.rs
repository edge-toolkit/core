//! Covers adding submissions to a registry file: new entries, new sightings of known ones, repeats, and every way a
//! submission or the file it is added to is refused.
#![cfg(test)]

use std::path::PathBuf;

use et_gpu_probe_server::registry::{Outcome, RegisterError, Registry, Submission};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A registry with one signature and one withheld interface, laid out as `libs/gpu-probe/signatures.yaml` is.
const REGISTRY: &str = r#"# The header, kept as it is.
probe_set: 1
signatures:
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
      multiply_add: "33800000"
      fma: "33800000"
  - interface: webgpu
    label: "Tor Browser"
    seen:
      - browser: tor-browser
        os: macos
        cpu_arch: aarch64
        gpu: apple
        backend: unknown
        window: normal
    blocked: true
    reason: "WebGPU is unavailable"
"#;

/// A registry file holding `text`, in a directory removed when the returned guard drops.
fn registry(text: &str) -> (TempDir, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signatures.yaml");
    fs_err::write(&path, text).unwrap();
    (dir, Registry::new(path))
}

/// A row of where a submission was seen.
fn row(browser: &str) -> Value {
    json!({
        "browser": browser,
        "browser_version": "157",
        "os": "linux",
        "cpu_arch": "x86_64",
        "gpu": "intel",
        "backend": "vulkan",
        "window": "normal",
    })
}

/// The submission `value` reads as.
fn submission(value: Value) -> Submission {
    serde_json::from_value(value).unwrap()
}

/// A signature submission through `interface`, with the results the JSON object `results` holds, seen in `browser`.
///
/// It is read from JSON text, as the server reads a request, since a `json!` value sorts its keys and so would lose the
/// order the results were given in, and any name given twice.
fn signature(interface: &str, results: &str, browser: &str) -> Submission {
    let seen = row(browser);
    let text = format!(r#"{{"interface": "{interface}", "label": "New GPU", "results": {results}, "seen": {seen}}}"#);
    serde_json::from_str(&text).unwrap()
}

/// The registry file's text.
fn text(registry: &Registry) -> String {
    fs_err::read_to_string(registry.path()).unwrap()
}

/// What adding `submission` to [`REGISTRY`] fails with, as the error words it.
fn refusal(submission: &Submission) -> RegisterError {
    let (_dir, registry) = registry(REGISTRY);
    registry.register(submission).unwrap_err()
}

#[test]
fn a_new_signature_is_appended_with_its_results_in_the_order_given() {
    let (_dir, registry) = registry(REGISTRY);
    let outcome = registry.register(&signature("webgpu-clear", r#"{"zeta": "1", "alpha": "2"}"#, "firefox"));
    assert_eq!(outcome.unwrap(), Outcome::Added(3));
    let expected = concat!(
        "  - interface: webgpu-clear\n",
        "    label: \"New GPU\"\n",
        "    seen:\n",
        "      - browser: firefox\n",
        "        browser_version: \"157\"\n",
        "        os: linux\n",
        "        cpu_arch: x86_64\n",
        "        gpu: intel\n",
        "        backend: vulkan\n",
        "        window: normal\n",
        "    results:\n",
        "      zeta: \"1\"\n",
        "      alpha: \"2\"\n",
    );
    assert_eq!(text(&registry), [REGISTRY, expected].concat());
}

#[test]
fn a_known_signature_seen_somewhere_new_gains_a_row_before_its_results() {
    let (_dir, registry) = registry(REGISTRY);
    let results = r#"{"fma": "33800000", "multiply_add": "33800000"}"#;
    let outcome = registry.register(&signature("webgpu-split", results, "firefox"));
    assert_eq!(outcome.unwrap(), Outcome::Sighted("Known GPU".to_owned()));
    let updated = text(&registry);
    let row = updated.find("      - browser: firefox\n").unwrap();
    let results = updated.find("    results:\n").unwrap();
    let blocked = updated.find("  - interface: webgpu\n").unwrap();
    assert!(row < results && results < blocked);
    assert!(updated.starts_with("# The header, kept as it is.\n"));
}

#[test]
fn a_row_the_registry_holds_with_more_fields_already_records_a_sighting() {
    let (_dir, registry) = registry(REGISTRY);
    let seen = [
        ("browser", "chrome"),
        ("os", "macos"),
        ("cpu_arch", "aarch64"),
        ("gpu", "apple"),
        ("backend", "metal"),
        ("window", "normal"),
    ];
    let results = vec![
        ("multiply_add".to_owned(), "33800000".to_owned()),
        ("fma".to_owned(), "33800000".to_owned()),
    ];
    let row = |extra: &[(&str, &str)]| {
        seen.iter()
            .chain(extra)
            .map(|(field, value)| ((*field).to_owned(), (*value).to_owned()))
            .collect()
    };

    // Fields the registry's row does not give make a sighting of its own.
    let richer = row(&[("device", "a laptop"), ("browser_version", "157")]);
    let outcome = registry.register(&Submission::signature(
        "webgpu-split",
        "ignored",
        results.clone(),
        richer,
    ));
    assert_eq!(outcome.unwrap(), Outcome::Sighted("Known GPU".to_owned()));
    let after = text(&registry);
    assert!(after.contains("        device: \"a laptop\"\n"));

    // A submission giving only some of that row's fields, all the same, is already on record.
    let partial = Submission::signature("webgpu-split", "ignored", results, row(&[("device", "a laptop")]));
    assert_eq!(
        registry.register(&partial).unwrap(),
        Outcome::Present("Known GPU".to_owned())
    );
    assert_eq!(text(&registry), after);
}

#[test]
fn a_repeat_of_a_known_row_changes_nothing() {
    let (_dir, registry) = registry(REGISTRY);
    let first = signature(
        "webgpu-split",
        r#"{"multiply_add": "33800000", "fma": "33800000"}"#,
        "firefox",
    );
    assert_eq!(
        registry.register(&first).unwrap(),
        Outcome::Sighted("Known GPU".to_owned())
    );
    let after_first = text(&registry);
    assert_eq!(
        registry.register(&first).unwrap(),
        Outcome::Present("Known GPU".to_owned())
    );
    assert_eq!(text(&registry), after_first);
}

#[test]
fn a_known_withheld_interface_seen_somewhere_new_gains_a_row_before_blocked() {
    let (_dir, registry) = registry(REGISTRY);
    let blocked = submission(json!({
        "interface": "webgpu", "label": "Tor Browser", "blocked": true, "reason": "absent", "seen": row("firefox")
    }));
    assert_eq!(
        registry.register(&blocked).unwrap(),
        Outcome::Sighted("Tor Browser".to_owned())
    );
    let updated = text(&registry);
    assert!(updated.find("      - browser: firefox\n").unwrap() < updated.find("    blocked: true\n").unwrap());
}

#[test]
fn a_new_withheld_interface_is_appended_with_its_reason() {
    let (_dir, registry) = registry(REGISTRY.trim_end());
    let blocked = submission(json!({
        "interface": "webgl2", "label": "Say \"no\"", "blocked": true, "reason": "a \\ b", "seen": row("firefox")
    }));
    assert_eq!(registry.register(&blocked).unwrap(), Outcome::Added(3));
    let updated = text(&registry);
    assert!(updated.contains("  reason: \"WebGPU is unavailable\"\n  - interface: webgl2\n"));
    assert!(updated.contains("    label: \"Say \\\"no\\\"\"\n"));
    assert!(updated.ends_with("    blocked: true\n    reason: \"a \\\\ b\"\n"));
}

#[test]
fn each_outcome_says_what_became_of_the_submission() {
    assert_eq!(Outcome::Added(3).to_string(), "added; the registry now holds 3 entries");
    assert_eq!(Outcome::Sighted("G".to_owned()).to_string(), "added a sighting to: G");
    assert_eq!(Outcome::Present("G".to_owned()).to_string(), "already present: G");
}

#[test]
fn a_submission_with_a_bad_interface_or_label_is_refused() {
    let bad_interface = signature("9webgpu", r#"{"a": "1"}"#, "firefox");
    assert!(matches!(refusal(&bad_interface), RegisterError::Interface));
    let long = signature(&"w".repeat(65), r#"{"a": "1"}"#, "firefox");
    assert!(matches!(refusal(&long), RegisterError::Interface));
    let unlabelled = submission(json!({"interface": "webgpu", "label": "", "results": {"a": "1"}, "seen": row("x")}));
    assert!(matches!(refusal(&unlabelled), RegisterError::Text(field) if field == "label"));
}

#[test]
fn a_signature_with_missing_or_unplain_results_is_refused() {
    for results in [
        "{}",
        r#"{"a": "has space"}"#,
        r#"{"a": ""}"#,
        r#"{"_a": "1"}"#,
        r#"{"a": "1", "a": "2"}"#,
    ] {
        let refused = refusal(&signature("webgpu", results, "firefox"));
        assert!(matches!(refused, RegisterError::Results));
        assert!(refused.is_rejection());
    }
    let many: Vec<String> = (0_u32..257).map(|index| format!(r#""r{index}": "1""#)).collect();
    let refused = refusal(&signature("webgpu", &format!("{{{}}}", many.join(", ")), "firefox"));
    assert!(matches!(refused, RegisterError::Results));
}

#[test]
fn a_withheld_interface_takes_a_reason_and_a_signature_none() {
    let with_results = submission(json!({
        "interface": "webgpu", "label": "L", "blocked": true, "reason": "r", "results": {"a": "1"}, "seen": row("x")
    }));
    assert!(matches!(refusal(&with_results), RegisterError::Blocked));
    let with_reason = submission(json!({
        "interface": "webgpu", "label": "L", "reason": "r", "results": {"a": "1"}, "seen": row("x")
    }));
    assert!(matches!(refusal(&with_reason), RegisterError::Blocked));
    let without_reason = submission(json!({
        "interface": "webgpu", "label": "L", "blocked": true, "seen": row("firefox")
    }));
    assert!(matches!(refusal(&without_reason), RegisterError::Text(field) if field == "reason"));
}

#[test]
fn a_row_the_registry_cannot_read_or_quote_is_refused() {
    let unknown = signature("webgpu", r#"{"a": "1"}"#, "netscape");
    assert!(matches!(refusal(&unknown), RegisterError::Row(_)));
    let mut multiline = row("firefox");
    multiline["device"] = json!("one\ntwo");
    let refused = refusal(&submission(json!({
        "interface": "webgpu", "label": "L", "results": {"a": "1"}, "seen": multiline
    })));
    assert!(matches!(&refused, RegisterError::Text(field) if field == "device"));
    assert_eq!(
        refused.to_string(),
        "device must be 1 to 200 characters with no control characters"
    );
}

#[test]
fn an_added_submission_leaves_no_staged_file_behind() {
    let (dir, registry) = registry(REGISTRY);
    let new = signature("webgpu-clear", r#"{"a": "1"}"#, "firefox");
    assert_eq!(registry.register(&new).unwrap(), Outcome::Added(3));
    assert!(!dir.path().join("signatures.yaml.partial").exists());
}

#[test]
fn a_staged_file_that_cannot_be_written_leaves_the_registry_as_it_was() {
    let (dir, registry) = registry(REGISTRY);
    // A directory where the staged file goes makes writing it fail, after the registry has been read.
    fs_err::create_dir(dir.path().join("signatures.yaml.partial")).unwrap();
    let new = signature("webgpu-clear", r#"{"a": "1"}"#, "firefox");
    assert!(matches!(registry.register(&new).unwrap_err(), RegisterError::Io(_)));
    assert_eq!(text(&registry), REGISTRY);
}

#[test]
fn a_registry_that_cannot_be_read_parsed_or_laid_out_is_not_the_submissions_fault() {
    let new = signature("webgpu-clear", r#"{"a": "1"}"#, "firefox");

    let missing = Registry::new(PathBuf::from("no-such-dir-for-et-gpu-probe-server/signatures.yaml"));
    let refused = missing.register(&new).unwrap_err();
    assert!(matches!(refused, RegisterError::Io(_)) && !refused.is_rejection());

    let (_dir, unparsable) = registry("signatures: [\n");
    assert!(matches!(
        unparsable.register(&new).unwrap_err(),
        RegisterError::Parse(_)
    ));

    let flow = "signatures:\n  - {interface: webgpu-clear, label: F, results: {a: \"1\"}, seen: []}\n";
    let (_dir, flowing) = registry(flow);
    let refused = flowing.register(&new).unwrap_err();
    assert!(matches!(refused, RegisterError::Layout(0)));
    assert!(!refused.is_rejection());
}

#[test]
fn a_seen_list_another_entry_aliases_is_not_added_to() {
    // The second entry's `seen` is an alias of the first's, so a row added to the first would read back in both.
    let shared = concat!(
        "signatures:\n",
        "  - interface: webgpu-clear\n    label: \"A\"\n    seen: &places\n",
        "      - browser: chrome\n        os: linux\n        cpu_arch: x86_64\n        gpu: intel\n",
        "        backend: vulkan\n        window: normal\n",
        "    results:\n      a: \"1\"\n",
        "  - interface: webgpu-clear\n    label: \"B\"\n    seen: *places\n    results:\n      a: \"2\"\n",
    );
    let (_dir, aliased) = registry(shared);
    let new = signature("webgpu-clear", r#"{"a": "1"}"#, "firefox");
    assert!(matches!(
        aliased.register(&new).unwrap_err(),
        RegisterError::Unwritable(0)
    ));
    assert_eq!(text(&aliased), shared);
}

#[test]
fn an_empty_list_the_text_cannot_be_added_to_leaves_the_registry_as_it_was() {
    let new = signature("webgpu-clear", r#"{"a": "1"}"#, "firefox");

    let (_dir, empty) = registry("signatures: []\n");
    assert!(matches!(
        empty.register(&new).unwrap_err(),
        RegisterError::Unwritable(0)
    ));
    assert_eq!(text(&empty), "signatures: []\n");

    let unseen =
        "signatures:\n  - interface: webgpu-clear\n    label: \"F\"\n    seen: []\n    results:\n      a: \"1\"\n";
    let (_dir, never_seen) = registry(unseen);
    let refused = never_seen.register(&new).unwrap_err();
    assert!(matches!(refused, RegisterError::Unwritable(0)) && !refused.is_rejection());
    assert_eq!(text(&never_seen), unseen);
}

#[test]
fn text_past_ascii_is_written_escaped_and_reads_back_the_same() {
    let (_dir, registry) = registry(REGISTRY);
    let label = "accent \u{e9} \u{1d11e} \"quoted\" back\\slash";
    let new = |browser: &str| {
        submission(json!({"interface": "webgpu-clear", "label": label, "results": {"a": "1"}, "seen": row(browser)}))
    };
    assert_eq!(registry.register(&new("firefox")).unwrap(), Outcome::Added(3));
    let written = text(&registry);
    assert!(written.is_ascii());
    // The label as the file holds it: each character past ASCII as its escape, then the quote and backslash escaped.
    let escaped = "label: \"accent \\u00e9 \\U0001d11e \\\"quoted\\\" back\\\\slash\"";
    assert!(written.contains(escaped), "{written}");
    // A second sighting finds the entry and names it by its label as read back from the file.
    assert_eq!(
        registry.register(&new("chrome")).unwrap(),
        Outcome::Sighted(label.to_owned())
    );
}

#[test]
fn writers_that_share_no_registry_value_lose_none_of_each_others_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signatures.yaml");
    fs_err::write(&path, REGISTRY).unwrap();
    // Each thread holds a registry of its own, as a second process would, so only the lock file keeps them apart.
    let writers: Vec<_> = (0_u32..8)
        .map(|writer| {
            let registry = Registry::new(path.clone());
            std::thread::spawn(move || {
                let mut seen = row("firefox");
                seen["browser_version"] = json!(format!("{writer}"));
                let results = json!({"multiply_add": "33800000", "fma": "33800000"});
                let sighting = json!({"interface": "webgpu-split", "label": "L", "results": results, "seen": seen});
                registry.register(&submission(sighting)).unwrap()
            })
        })
        .collect();
    for writer in writers {
        assert!(matches!(writer.join().unwrap(), Outcome::Sighted(_)));
    }
    let after = fs_err::read_to_string(&path).unwrap();
    for writer in 0_u32..8 {
        assert!(
            after.contains(&format!("browser_version: \"{writer}\"")),
            "row {writer} was lost"
        );
    }
}

#[test]
fn a_submission_carrying_a_field_the_registry_does_not_know_does_not_read() {
    let stray = json!({"interface": "webgpu", "label": "L", "results": {}, "seen": {}, "colour": "red"});
    let refused = serde_json::from_value::<Submission>(stray).unwrap_err();
    assert!(refused.to_string().contains("colour"));
    let not_a_map = json!({"interface": "webgpu", "label": "L", "results": ["a"], "seen": {}});
    let refused = serde_json::from_value::<Submission>(not_a_map).unwrap_err();
    assert!(refused.to_string().contains("a map of result names to values"));
}
