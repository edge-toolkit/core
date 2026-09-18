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

/// Poll until the probe has genuinely exited, giving up after ~30s.
///
/// The probe exits immediately, but "immediately" still means once the OS has got round to scheduling it, and a
/// bare assertion would race that on a loaded runner. Returning quietly on the timeout rather than asserting
/// leaves the caller to say what it expected, so each failure reads as the thing that test was about.
fn wait_until_gone(guard: &mut ChildGuard) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) && !guard.has_exited() {
        std::thread::sleep(Duration::from_millis(50));
    }
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

    wait_until_gone(&mut guard);

    assert!(
        guard.has_exited(),
        "a probe asked to sleep for 0ms should read as exited well inside the bound"
    );
}

/// A child that exited without the polling loop noticing is still reported as having exited on its own.
///
/// The loop sleeps between polls, so a child can exit inside that gap and the loop run out of budget without
/// ever seeing it; the look after the loop is what catches that, and reporting it as a kill would tell the
/// caller the process had to be forced when it finished by itself. Racing a real sleep would be the obvious way
/// to arrive there and a flaky one -- a zero budget reaches the same branch every time, because the loop cannot
/// run at all and the look after it is the only thing left to decide the answer.
#[test]
fn wait_for_exit_credits_a_child_the_loop_never_saw_finish() {
    let mut guard = spawn_probe(0);

    // Waited out first so the probe is genuinely gone before the zero-budget call, which otherwise reaches the
    // same branch and answers "still running" simply because the OS had not scheduled the exit yet.
    wait_until_gone(&mut guard);

    assert!(
        guard.wait_for_exit(Duration::ZERO),
        "a child that has already exited must read as exited even with no time left to look for it"
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
