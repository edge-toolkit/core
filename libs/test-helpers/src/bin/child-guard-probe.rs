//! Test fixture for `ChildGuard::wait_for_exit`: sleeps for its argument in milliseconds, then exits.
//!
//! The two outcomes that method distinguishes -- a child that ends by itself and one that has to be killed --
//! are only tellable apart by controlling how long the child lives, and no binary that takes a sleep duration
//! exists on all five supported platforms. Building the fixture here means the test can name it through
//! `CARGO_BIN_EXE_child-guard-probe` and get the same behaviour on every one of them.

use std::time::Duration;

fn main() {
    // A missing or unparsable argument sleeps for zero, so a badly spawned probe exits rather than hanging.
    let millis = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<u64>().ok())
        .unwrap_or_default();
    std::thread::sleep(Duration::from_millis(millis));
}
