#![cfg(test)]
#![expect(
    clippy::unwrap_used,
    reason = "test code: failed tempdir/fs setup should fail the test"
)]

use et_path::find_project_root;
use fs_err as fs;
use tempfile::tempdir;

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
