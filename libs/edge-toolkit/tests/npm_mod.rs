//! Layout-agnostic tests for `find_npm_modules_path_in` — the
//! pure-filesystem core of `mise_npm_modules_path`. Each fixture
//! constructs a tempdir mimicking one of the mise npm backends and
//! verifies the resolver picks the right `node_modules` directory.

#![cfg(test)]
#![expect(clippy::unwrap_used, reason = "test code: failed tempdir setup should fail the test")]

use std::fs;

use edge_toolkit::config::find_npm_modules_path_in;
use tempfile::TempDir;

#[test]
fn resolves_classical_npm_layout() {
    // <install>/lib/node_modules/onnxruntime-web/package.json
    let install = TempDir::new().unwrap();
    let modules = install.path().join("lib/node_modules");
    let package = modules.join("onnxruntime-web");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), "{}").unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert_eq!(found.as_deref(), Some(modules.as_path()));
}

#[test]
fn resolves_aube_backend_layout() {
    // <install>/global-aube/<content-hash>/node_modules/.aube/node_modules/onnxruntime-web/
    //
    // The `<content-hash>` segment is opaque to the resolver — it
    // walks whatever's under `global-aube/`, so any subdir name works.
    // A clearly-synthetic placeholder makes it obvious we're not
    // pinning the test to one developer's local install.
    let install = TempDir::new().unwrap();
    let modules = install
        .path()
        .join("global-aube/test-hash/node_modules/.aube/node_modules");
    let package = modules.join("onnxruntime-web");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), "{}").unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert_eq!(found.as_deref(), Some(modules.as_path()));
}

#[test]
fn returns_none_when_neither_layout_has_the_package() {
    let install = TempDir::new().unwrap();
    // Make `lib/node_modules` exist but with a *different* package, so
    // the classical layout's directory check passes but the package
    // check fails — exercises the "is the named dir present" path.
    let modules = install.path().join("lib/node_modules");
    fs::create_dir_all(modules.join("some-other-package")).unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert!(found.is_none());
}

#[test]
fn classical_layout_wins_if_both_exist() {
    // Construct both layouts. Classical is tried first, so it should
    // win — keeps the resolver deterministic when a user has migrated
    // between backends without cleaning up.
    let install = TempDir::new().unwrap();
    let classical = install.path().join("lib/node_modules");
    fs::create_dir_all(classical.join("onnxruntime-web")).unwrap();
    let aube = install.path().join("global-aube/abc/node_modules/.aube/node_modules");
    fs::create_dir_all(aube.join("onnxruntime-web")).unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert_eq!(found.as_deref(), Some(classical.as_path()));
}
