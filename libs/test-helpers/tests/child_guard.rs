//! Covers both outcomes of `ChildGuard::wait_for_exit` and of `has_exited` against a child whose lifetime the
//! test picks.
#![cfg(test)]

use std::process::Command;
use std::time::{Duration, Instant};

use command_error::CommandExt as _;
use et_test_helpers::ChildGuard;

/// Spawn the probe binary so it lives for `millis`, under a guard.
fn spawn_probe(millis: u64) -> ChildGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_child-guard-probe"))
        .arg(millis.to_string())
        .spawn_checked()
        .unwrap()
        .into_child();
    ChildGuard::new(child)
}

#[test]
fn wait_for_exit_reports_a_child_that_ends_on_its_own() {
    let mut guard = spawn_probe(0);

    // Generous next to a probe that exits immediately: the bound is only reached if the child hangs, and a
    // loaded CI runner can take a while to get round to scheduling it.
    assert!(
        guard.wait_for_exit(Duration::from_secs(30)),
        "a probe asked to sleep for 0ms should exit well inside the timeout"
    );
}

#[test]
fn wait_for_exit_kills_a_child_that_overstays() {
    let mut guard = spawn_probe(600_000);

    assert!(
        !guard.wait_for_exit(Duration::from_millis(500)),
        "a probe asked to sleep for ten minutes should still be running when the timeout passes"
    );
}

#[test]
fn has_exited_reports_a_child_that_has_ended() {
    let mut guard = spawn_probe(0);

    // Polled rather than asserted once: the probe exits immediately, but "immediately" still means once the OS
    // has got round to scheduling it, and a bare assertion would race that on a loaded runner.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) && !guard.has_exited() {
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        guard.has_exited(),
        "a probe asked to sleep for 0ms should read as exited well inside the bound"
    );
}

#[test]
fn has_exited_leaves_a_live_child_running() {
    let mut guard = spawn_probe(600_000);

    assert!(
        !guard.has_exited(),
        "a probe asked to sleep for ten minutes has not exited"
    );
    // Asked twice on purpose: the point of this helper over `wait_for_exit` is that it neither waits the child
    // out nor kills it, so a second look must still find it running.
    assert!(!guard.has_exited(), "has_exited must not itself end the child");
}
