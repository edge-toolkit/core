//! Covers `generate_zig`'s "openapi2zig not on PATH" skip branch.
//!
//! `generate_zig` probes for the `openapi2zig` binary via [`zig::is_available`] and quietly skips Zig REST-client
//! generation when it is absent (upstream ships no linux/arm64 release, so that lane always takes this path). We
//! force the skip regardless of whether the tool is installed on the test runner by emptying `PATH` for the
//! duration of the call with `temp-env`, so the internal `Command::new("openapi2zig")` lookup finds nothing.
#![cfg(test)]

use et_int_gen::zig;

#[test]
fn generate_zig_skips_when_openapi2zig_is_absent() {
    // `with_empty_path` empties the real process `PATH` for the closure and restores it after, so the OS-level
    // program resolution inside `is_available` sees an empty `PATH` and can't find openapi2zig.
    et_test_helpers::with_empty_path(|| {
        assert!(!zig::is_available(), "openapi2zig must look absent with an empty PATH");
        // The skip branch prints a notice and returns `Ok(())` without invoking openapi2zig or touching the
        // committed client, so this succeeds even on a runner that has the tool installed.
        let result = et_int_gen::generate_zig();
        assert!(
            result.is_ok(),
            "generate_zig must succeed by skipping when openapi2zig is absent: {result:?}"
        );
    });
}
