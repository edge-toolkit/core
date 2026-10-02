//! Covers `write_if_changed`: it creates missing parents, replaces changed contents, and leaves no staged file behind.
#![cfg(test)]

use et_int_gen::write_if_changed;
use fs_err as fs;
use tempfile::TempDir;

/// The names directly inside `dir`, sorted.
fn entries(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_missing_file_is_written_with_its_parents() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("nested/dir/out.txt");

    write_if_changed(&path, "first\n").unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "first\n");
    assert_eq!(entries(&root.path().join("nested/dir")), ["out.txt"]);
}

#[test]
fn changed_contents_replace_the_file() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("out.txt");
    fs::write(&path, "old\n").unwrap();

    write_if_changed(&path, "new\n").unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
    assert_eq!(entries(root.path()), ["out.txt"]);
}

#[test]
fn unchanged_contents_leave_the_file_alone() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("out.txt");
    fs::write(&path, "same\n").unwrap();

    write_if_changed(&path, "same\n").unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "same\n");
    assert_eq!(entries(root.path()), ["out.txt"]);
}
