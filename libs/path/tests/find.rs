#![cfg(test)]

use et_path::{emit_repo_file_env, find_project_root, find_project_root_from_manifest, repo_file_env_directives};
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

#[test]
fn repo_file_env_directives_name_the_var_the_file_and_both_rerun_triggers() {
    let relative = "services/ws-test-server/data/math1-input.json";
    let [set_var, rerun_build, rerun_file] = repo_file_env_directives("ET_PROBE_PATH", relative);

    // The absolute path is the repo root joined with the relative one, so both directives that carry a path
    // end in it. Asserting the suffix rather than the whole string keeps the test independent of where the
    // checkout lives.
    let expected_suffix = relative.replace('/', std::path::MAIN_SEPARATOR_STR);
    assert!(
        set_var.starts_with("cargo:rustc-env=ET_PROBE_PATH=") && set_var.ends_with(&expected_suffix),
        "unexpected rustc-env directive: {set_var}"
    );
    assert_eq!(rerun_build, "cargo:rerun-if-changed=build.rs");
    assert!(
        rerun_file.starts_with("cargo:rerun-if-changed=") && rerun_file.ends_with(&expected_suffix),
        "unexpected rerun-if-changed directive: {rerun_file}"
    );

    // Run the emitter over the same inputs. Its output goes to stdout, which a test cannot read back from
    // inside its own process, so the assertions above are what pin the text; this covers the one thing they
    // cannot -- that the emitter still goes through the function that produces it, and does so without panicking.
    emit_repo_file_env("ET_PROBE_PATH", relative);
}
