//! Verifies `default_modules_folders` degrades gracefully when `mise`
//! isn't on PATH: it returns just the hardcoded workspace paths
//! and emits no warning log records. Without this, every deployment
//! that doesn't use mise would see misleading `mise install ...`
//! warnings at startup.

#![cfg(test)]

use std::path::PathBuf;

use edge_toolkit::config::default_modules_folders;

#[test]
fn returns_only_workspace_paths_when_mise_missing() {
    // Initialise the log capture *before* hiding mise so the recorder
    // is attached to the global `log` facade for this thread. The
    // crate uses a thread-local buffer, so this is safe to call from
    // a parallel test runner.
    testing_logger::setup();

    // Empty PATH for the call, hiding mise (and every other binary) from the spawn in `mise_is_available`.
    // `with_empty_path` restores PATH after the closure, so we don't poison sibling tests in the same binary.
    let paths = et_test_helpers::with_empty_path(default_modules_folders);

    // Hardcoded workspace paths only, zero mise-resolved paths.
    let expected_suffixes = [
        "ws-server/static",
        "services/ws-wasm-agent",
        "data/model-modules",
        "services/ws-modules",
        "generated/python-ws",
        "generated/python-rest",
    ];
    assert_eq!(
        paths.len(),
        expected_suffixes.len(),
        "expected only the {} workspace paths when mise is unavailable, got {paths:?}",
        expected_suffixes.len(),
    );

    // Each returned path is under the project root and matches one of
    // the constants the function pushes unconditionally. We don't pin
    // exact strings because `get_project_root` is host-dependent --
    // just check the suffixes are right.
    let suffixes: Vec<PathBuf> = paths
        .iter()
        .map(|path| {
            path.components()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .iter()
                .rev()
                .collect::<PathBuf>()
        })
        .collect();
    for expected in expected_suffixes {
        let expected_path = PathBuf::from(expected);
        assert!(
            suffixes
                .iter()
                .any(|suffix| suffix.ends_with(&expected_path) || suffix == &expected_path),
            "expected a path ending in {expected:?}, got {paths:?}",
        );
    }

    // Zero log records emitted. If any warning fires here it'd be
    // confusing user-visible noise on a no-mise deployment.
    testing_logger::validate(|records| {
        assert!(
            records.is_empty(),
            "expected no log records when mise is unavailable, got {:?}",
            records
                .iter()
                .map(|record| (record.level, record.body.as_str()))
                .collect::<Vec<_>>(),
        );
    });
}
