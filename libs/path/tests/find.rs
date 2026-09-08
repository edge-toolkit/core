#![cfg(test)]

use std::path::Path;

use et_path::{emit_repo_file_env, find_project_root, find_project_root_from_manifest, repo_file_env_directives};
use fs_err as fs;
use tempfile::tempdir;

/// Return the path a cargo directive carries, failing the test if it does not start with `prefix`.
fn directive_path<'directive>(directive: &'directive str, prefix: &str) -> &'directive str {
    directive
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("directive should start with `{prefix}`: {directive}"))
}

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

    // The absolute path is the repo root joined with the relative one, so both directives that carry a path end
    // in `relative` verbatim -- forward slashes included. `Path::join` appends a `/`-separated string as it
    // stands rather than rewriting it to the platform separator, so on Windows the value is a mixed
    // `D:\checkout\services/ws-test-server/data/math1-input.json`, which every path API there accepts. Expecting
    // the separator to be rewritten failed on all three Windows lanes of commit
    // 29dfe80a62ba7a27d8119c5b6332c3dbe2df815e with
    //
    //     unexpected rustc-env directive:
    //     cargo:rustc-env=ET_PROBE_PATH=D:\a\core\core\services/ws-test-server/data/math1-input.json
    //
    // e.g. https://github.com/edge-toolkit/core/actions/runs/34211905976/job/102014621935. Asserting the suffix
    // rather than the whole string keeps the test independent of where the checkout lives, and `is_absolute` is
    // what pins the root having been prepended at all.
    let set_path = directive_path(&set_var, "cargo:rustc-env=ET_PROBE_PATH=");
    assert!(
        Path::new(set_path).is_absolute(),
        "should be an absolute path: {set_path}"
    );
    assert!(
        set_path.ends_with(relative),
        "unexpected rustc-env directive: {set_var}"
    );
    assert_eq!(rerun_build, "cargo:rerun-if-changed=build.rs");
    let rerun_path = directive_path(&rerun_file, "cargo:rerun-if-changed=");
    assert!(
        Path::new(rerun_path).is_absolute(),
        "should be an absolute path: {rerun_path}"
    );
    assert_eq!(rerun_path, set_path, "both directives should name the same file");

    // Run the emitter over the same inputs. Its output goes to stdout, which a test cannot read back from
    // inside its own process, so the assertions above are what pin the text; this covers the one thing they
    // cannot -- that the emitter still goes through the function that produces it, and does so without panicking.
    emit_repo_file_env("ET_PROBE_PATH", relative);
}
