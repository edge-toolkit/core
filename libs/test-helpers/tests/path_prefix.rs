//! Covers `with_path_prefix`: a directory it puts in front of `PATH` resolves bare names, and only inside.
//!
//! Asserted by spawning rather than by inspecting the variable, because resolution is the whole contract --
//! the helper exists so a stand-in shadows a real tool, and the OS's own program lookup is the only thing that
//! decides whether it does. Reading `PATH` back and matching on its text would assert the implementation.
#![cfg(test)]

use std::process::Command;

use command_error::CommandExt as _;
use fs_err as fs;
use tempfile::TempDir;

/// Bare name the probe copy is invoked by, distinctive enough that nothing on a real `PATH` answers to it.
const PROBE_NAME: &str = "et-path-prefix-probe";

/// A directory holding a copy of the child-guard probe, named [`PROBE_NAME`].
///
/// Copied rather than compiled: the probe is already built for this crate's tests on every supported platform,
/// so reusing it keeps the fixture to a file copy and needs no toolchain at test time. The `.exe` suffix is
/// what makes the copy executable on Windows, where program lookup is extension-driven.
fn dir_holding_the_probe() -> TempDir {
    let dir = TempDir::new().unwrap();
    let name = if cfg!(windows) {
        format!("{PROBE_NAME}.exe")
    } else {
        PROBE_NAME.to_owned()
    };
    let _bytes: u64 = fs::copy(env!("CARGO_BIN_EXE_child-guard-probe"), dir.path().join(name)).unwrap();
    dir
}

/// Spawn the probe by bare name, reporting whether program lookup found it.
///
/// `0` so a copy that is found exits immediately instead of holding the test open.
fn probe_resolves() -> bool {
    Command::new(PROBE_NAME).arg("0").output().is_ok()
}

#[test]
fn a_prefixed_directory_resolves_bare_names_inside_the_closure() {
    let dir = dir_holding_the_probe();

    assert!(
        et_test_helpers::with_path_prefix(dir.path(), probe_resolves),
        "a directory put in front of PATH must make its executables resolve by bare name"
    );
}

#[test]
fn the_prefix_is_gone_once_the_closure_returns() {
    let dir = dir_holding_the_probe();

    // Asserted first so a failure here reads as "the prefix never worked" rather than "it was not restored":
    // the check below passes trivially if the directory was never on PATH to begin with.
    assert!(
        et_test_helpers::with_path_prefix(dir.path(), probe_resolves),
        "the probe must resolve inside the closure, or the restoration check below proves nothing"
    );
    assert!(
        !probe_resolves(),
        "PATH must be restored when the closure returns, so the prefixed directory stops resolving"
    );
}

#[test]
fn the_inherited_path_survives_behind_the_prefixed_directory() {
    let dir = dir_holding_the_probe();

    // Prefixing rather than replacing is the whole reason this helper exists: an executable the toolchain has
    // just built may still resolve part of its runtime through the inherited PATH, and cutting PATH down to
    // the stand-in's own directory left one unable to start on the x64 Windows lanes. A mise-managed tool is
    // what proves the inherited entries are still there -- every sanctioned environment has them on PATH, so
    // a failure here is a real regression rather than a machine that happens to lack the tool.
    et_test_helpers::with_path_prefix(dir.path(), || {
        let _ran: std::process::Output = Command::new("coreutils").arg("--version").output_checked().unwrap();
    });
}
