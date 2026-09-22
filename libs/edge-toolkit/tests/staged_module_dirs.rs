//! Turning a `mise ls --current --json` payload into the module directories the hub serves.
//!
//! Every rejection here is silent by design: a deployment names its modules as `[tools]`, and one that is missing, of
//! another backend, or laid out in a way this does not recognise simply contributes nothing. The hub then reports it
//! as a module it cannot find, which is the diagnosis the operator needs -- as opposed to the server refusing to start
//! because one entry in a tool listing was not what it expected.
#![cfg(test)]

use edge_toolkit::config::{mise_staged_module_dirs, staged_module_dirs_from_tool_list};
use fs_err as fs;
use tempfile::TempDir;

/// Build an npm install whose package really sits at `<install>/node_modules/<package>`.
fn staged_install(root: &TempDir, install: &str, package: &str) -> std::path::PathBuf {
    let install_dir = root.path().join(install);
    fs::create_dir_all(install_dir.join("node_modules").join(package)).unwrap();
    install_dir
}

#[test]
fn a_tool_list_that_is_not_json_yields_no_directories() {
    // An older mise, a wrapper that printed a warning first, a truncated pipe. The hub has to come away with nothing
    // rather than fail to start, so it serves what it can and says what is missing.
    assert!(staged_module_dirs_from_tool_list(b"mise: not a tool list").is_empty());
    assert!(
        staged_module_dirs_from_tool_list(b"").is_empty(),
        "an empty body is not valid JSON either"
    );
}

#[test]
fn a_listing_that_is_not_an_object_yields_no_directories() {
    // Valid JSON, wrong shape: mise answers with a map of tool id to versions, and anything else is a version of mise
    // this does not know how to read.
    assert!(staged_module_dirs_from_tool_list(b"[]").is_empty());
    assert!(staged_module_dirs_from_tool_list(b"\"a string\"").is_empty());
}

#[test]
fn only_npm_tools_whose_package_is_present_contribute_directories() {
    // A mixed tool set, which is what a real config produces. Only the npm tool whose install actually holds the
    // package contributes: a tool of another backend is not a module, and an npm install missing its package would put
    // a directory with no `package.json` in front of the module scan.
    let root = TempDir::new().unwrap();
    let present = staged_install(&root, "math1-install", "et-ws-math1");
    let missing = root.path().join("bare-install");
    fs::create_dir_all(&missing).unwrap();

    let tool_list = serde_json::json!({
        "npm:et-ws-math1": [{ "install_path": present }],
        "npm:et-ws-absent": [{ "install_path": missing }],
        "cargo:et-ws-server": [{ "install_path": present }],
    });
    let found = staged_module_dirs_from_tool_list(&serde_json::to_vec(&tool_list).unwrap());

    assert_eq!(found, vec![present.join("node_modules").join("et-ws-math1")]);
}

#[test]
fn a_scoped_package_resolves_to_its_own_directory() {
    // The scope is part of the path, so the package dir is `node_modules/@scope/name` -- the scope directory itself
    // holds no `package.json` and would be served as nothing.
    let root = TempDir::new().unwrap();
    let install = staged_install(&root, "scoped-install", "@edge-toolkit/et-ws-math1");

    let tool_list = serde_json::json!({ "npm:@edge-toolkit/et-ws-math1": [{ "install_path": install }] });
    let found = staged_module_dirs_from_tool_list(&serde_json::to_vec(&tool_list).unwrap());

    assert_eq!(
        found,
        vec![install.join("node_modules").join("@edge-toolkit/et-ws-math1")]
    );
}

#[test]
fn an_entry_with_no_version_or_no_install_path_is_skipped() {
    // Both shapes mise can produce for a tool it knows of but has not installed.
    let tool_list = serde_json::json!({
        "npm:et-ws-never-installed": [],
        "npm:et-ws-no-path": [{ "active": true }],
        "npm:et-ws-path-not-a-string": [{ "install_path": 42_i32 }],
    });

    assert!(staged_module_dirs_from_tool_list(&serde_json::to_vec(&tool_list).unwrap()).is_empty());
}

#[test]
fn no_mise_to_ask_contributes_no_directories() {
    // The other half of the lookup: with no `mise` to run there is no listing to interpret, and a deployment that
    // staged nothing has to reach the same empty answer as one whose listing named nothing. An empty PATH hides the
    // binary from the spawn, and `with_empty_path` puts PATH back so sibling tests in this binary still find it.
    assert!(et_test_helpers::with_empty_path(mise_staged_module_dirs).is_empty());
}
