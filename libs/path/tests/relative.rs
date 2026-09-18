//! Covers `relative_path_from`'s no-op case, where the two paths normalise to the same components.
//!
//! Every other caller in the repo passes a target below or beside `from_dir`, so the branch that has nothing
//! to join is only reachable from a test. It has to render as the current-directory component rather than the
//! empty string: the result is written verbatim into generated `mise.toml` / `docker-compose.yaml` output,
//! where an empty path is not a directory reference at all.
#![cfg(test)]

use std::path::{Component, Path};

use et_path::relative_path_from;

/// What the function must render when there is nothing to join.
///
/// Spelled through the path component rather than as a bare string, so the expectation says which path
/// element is meant instead of repeating the character the rendering happens to use.
fn current_dir() -> String {
    Component::CurDir.as_os_str().to_string_lossy().into_owned()
}

#[test]
fn identical_paths_render_as_the_current_directory() {
    let dir = Path::new("/workspace/services/ws-server");
    assert_eq!(relative_path_from(dir, dir), current_dir());
}

#[test]
fn paths_differing_only_in_normalisation_still_render_as_the_current_directory() {
    // Interior `.` and `..` segments are normalised away before the comparison, so these two spellings of one
    // directory take the same branch as the literally-identical pair above.
    let from = Path::new("/workspace/services/ws-server");
    let target = Path::new("/workspace/services/modules/../ws-server");
    assert_eq!(relative_path_from(from, target), current_dir());
}

#[test]
fn a_target_below_the_base_still_renders_its_suffix() {
    // The neighbouring case, kept here so the empty-parts branch is not asserted in isolation: a real
    // relative path must still come back joined with forward slashes on every host.
    let from = Path::new("/workspace");
    let target = Path::new("/workspace/services/ws-server");
    assert_eq!(relative_path_from(from, target), "services/ws-server");
}
