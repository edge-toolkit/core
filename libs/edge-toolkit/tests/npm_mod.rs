//! Layout-agnostic tests for `find_npm_modules_path_in` -- the
//! pure-filesystem core of `mise_npm_modules_path`. Each fixture
//! constructs a tempdir mimicking one of the mise npm backends and
//! verifies the resolver picks the right `node_modules` directory.

#![cfg(test)]

use edge_toolkit::config::find_npm_modules_path_in;
use fs_err as fs;
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
fn resolves_windows_npm_layout() {
    // npm on Windows installs globals to <install>/node_modules/<package>
    // (no `lib/` segment), unlike the Unix `lib/node_modules` layout.
    let install = TempDir::new().unwrap();
    let modules = install.path().join("node_modules");
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
    // The `<content-hash>` segment is opaque to the resolver -- it
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

/// Current mise reaches the package through two symlink hops, and the resolver must follow both.
///
/// `<install>/node_modules/<pkg>` is a *relative* symlink to
/// `.mise/<pkg>@<version>/node_modules/<pkg>`, and `.mise/<pkg>@<version>` is itself a symlink into the shared
/// aube virtual store. The resolver's single `is_dir` probe is what traverses the chain, so a regression to a
/// non-following check (`symlink_metadata`, `read_dir` entry filtering) would resolve nothing and every
/// `/modules/<pkg>/...` request would 404. The real layouts these hops mirror are recorded on
/// `find_npm_modules_path_in`.
///
/// Unix-only because it has to *create* symlinks: Windows symlink creation needs Developer Mode or elevation,
/// which isn't dependable on a runner. `services/modules/tests/symlinks.rs` is `#![cfg(unix)]` for the same
/// reason. The resolver code under test is platform-independent.
#[cfg(unix)]
#[test]
fn resolves_mise_two_hop_symlink_layout() {
    use std::os::unix::fs::symlink;

    let install = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let modules = install.path().join("node_modules");
    let farm = modules.join(".mise");
    fs::create_dir_all(&farm).unwrap();

    // Hop 2 target: the virtual-store entry holding the real package.
    let store_entry = store.path().join("onnxruntime-web@1.27.0-ac0bad64e3fabd3b");
    let real_pkg = store_entry.join("node_modules/onnxruntime-web");
    fs::create_dir_all(&real_pkg).unwrap();
    fs::write(real_pkg.join("package.json"), "{}").unwrap();

    symlink(&store_entry, farm.join("onnxruntime-web@1.27.0")).unwrap();
    symlink(
        std::path::Path::new(".mise/onnxruntime-web@1.27.0/node_modules/onnxruntime-web"),
        modules.join("onnxruntime-web"),
    )
    .unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert_eq!(found.as_deref(), Some(modules.as_path()));
    assert!(
        found.unwrap().join("onnxruntime-web/package.json").is_file(),
        "package.json must be readable through both symlink hops",
    );
}

#[test]
fn resolves_scoped_package_layout() {
    // A scoped npm package (e.g. @mediapipe/tasks-vision) lives at
    // <install>/lib/node_modules/@mediapipe/tasks-vision. The resolver joins the full "@scope/pkg" name, so it
    // returns the node_modules dir; mise_npm_package_path then appends the name to get the package dir served
    // as one module.
    let install = TempDir::new().unwrap();
    let modules = install.path().join("lib/node_modules");
    let package = modules.join("@mediapipe/tasks-vision");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), "{}").unwrap();

    let found = find_npm_modules_path_in(install.path(), "@mediapipe/tasks-vision");
    assert_eq!(found.as_deref(), Some(modules.as_path()));
    assert!(
        found
            .unwrap()
            .join("@mediapipe/tasks-vision")
            .join("package.json")
            .is_file()
    );
}

#[test]
fn returns_none_when_neither_layout_has_the_package() {
    let install = TempDir::new().unwrap();
    // Make `lib/node_modules` exist but with a *different* package, so
    // the classical layout's directory check passes but the package
    // check fails -- exercises the "is the named dir present" path.
    let modules = install.path().join("lib/node_modules");
    fs::create_dir_all(modules.join("some-other-package")).unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert!(found.is_none());
}

#[test]
fn classical_layout_wins_if_both_exist() {
    // Construct both layouts. Classical is tried first, so it should
    // win -- keeps the resolver deterministic when a user has migrated
    // between backends without cleaning up.
    let install = TempDir::new().unwrap();
    let classical = install.path().join("lib/node_modules");
    fs::create_dir_all(classical.join("onnxruntime-web")).unwrap();
    let aube = install.path().join("global-aube/abc/node_modules/.aube/node_modules");
    fs::create_dir_all(aube.join("onnxruntime-web")).unwrap();

    let found = find_npm_modules_path_in(install.path(), "onnxruntime-web");
    assert_eq!(found.as_deref(), Some(classical.as_path()));
}
