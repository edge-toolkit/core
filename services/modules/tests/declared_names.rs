//! A module is served under the name its `package.json` declares, whatever that name is.
//!
//! The server reshapes nothing. It has no notion of whose scope is whose, so it cannot privilege one
//! project's packages over another's -- a deployment run by anyone gets the same treatment, and the name a
//! module is published under is the name it is served, resolved and referred to by throughout.
#![cfg(test)]

use std::path::PathBuf;

use et_modules_service::{ModulesConfig, list_modules};
use fs_err as fs;
use tempfile::TempDir;

/// Write `<root>/<dir>/package.json` declaring `name`.
fn write_module(root: &TempDir, dir: &str, name: &str) {
    let module_dir = root.path().join(dir);
    fs::create_dir_all(&module_dir).unwrap();
    fs::write(
        module_dir.join("package.json"),
        format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
    )
    .unwrap();
}

fn discovered(root: &TempDir) -> Vec<String> {
    let config = ModulesConfig::new(vec![root.path().to_path_buf()], "unused-root".to_string());
    let mut names: Vec<String> = list_modules(&config).into_iter().map(|(name, _)| name).collect();
    names.sort();
    names
}

#[test]
fn a_scoped_name_is_served_exactly_as_declared() {
    let root = TempDir::new().unwrap();
    write_module(&root, "math1", "@edge-toolkit/et-ws-math1");

    assert_eq!(discovered(&root), vec!["@edge-toolkit/et-ws-math1".to_string()]);
}

#[test]
fn no_scope_is_privileged_over_another() {
    // The whole point: this server is not one project's. Two scopes go in, both come out untouched, and
    // nothing here knows which of them published the server it is running.
    let root = TempDir::new().unwrap();
    write_module(&root, "ours", "@edge-toolkit/et-ws-math1");
    write_module(&root, "theirs", "@huggingface/transformers");

    assert_eq!(
        discovered(&root),
        vec![
            "@edge-toolkit/et-ws-math1".to_string(),
            "@huggingface/transformers".to_string(),
        ]
    );
}

#[test]
fn an_unscoped_name_is_served_exactly_as_declared() {
    let root = TempDir::new().unwrap();
    write_module(&root, "agent", "et-ws-wasm-agent");

    assert_eq!(discovered(&root), vec!["et-ws-wasm-agent".to_string()]);
}

#[test]
fn a_configured_path_that_does_not_exist_is_skipped_rather_than_breaking_the_scan() {
    // A deployment naming a directory that is not there is a configuration error, but it must not take the
    // rest of the scan with it: the modules that are present still get served.
    let root = TempDir::new().unwrap();
    write_module(&root, "math1", "@edge-toolkit/et-ws-math1");

    let config = ModulesConfig::new(
        vec![PathBuf::from("/definitely/not/a/directory"), root.path().to_path_buf()],
        "unused-root".to_string(),
    );
    let names: Vec<String> = list_modules(&config).into_iter().map(|(name, _)| name).collect();

    assert_eq!(names, vec!["@edge-toolkit/et-ws-math1".to_string()]);
}
