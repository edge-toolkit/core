#![cfg(test)]

use et_path::{find_project_root, find_project_root_from_manifest};
use fs_err as fs;
use tempfile::tempdir;

#[test]
fn manifest_dir_resolves_to_an_existing_root() {
    // cargo sets `CARGO_MANIFEST_DIR` in the test process env, so the helper reads this crate's
    // directory and walks up to the workspace root (which carries project-root markers).
    let root = find_project_root_from_manifest();
    assert!(
        root.is_dir(),
        "resolved project root {root:?} should be an existing directory"
    );
}

#[test]
fn finds_marker_in_an_ancestor() {
    let root = tempdir().unwrap();
    fs::write(root.path().join(".dprint.jsonc"), "{}").unwrap();
    let nested = root.path().join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    // Canonicalize both sides so the macOS /var -> /private/var symlink on the
    // tempdir doesn't fail an otherwise-correct match.
    let found = fs::canonicalize(find_project_root(&nested)).unwrap();
    assert_eq!(found, fs::canonicalize(root.path()).unwrap());
}

#[test]
fn falls_back_to_start_when_marker_is_absent() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("x");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(find_project_root(&nested), nested);
}
