//! Coverage dump for the instrumented build, isolated into its own file.
//!
//! minicov's `capture_coverage` is an `unsafe fn` (it reads the raw instrumented counter buffers), which
//! Codacy flags for audit. Codacy can only exclude whole paths in-repo, not suppress per line, so this
//! one-function file is the single thing excluded from Codacy (see .codacy.yaml) while the rest of the guest
//! stays analyzed. The unsafe is still covered by the repo's own clippy (the crate expects `unsafe_code`) and
//! by DeepSource. Called from `run()` at the end; writes the profile to the runner's `/cov` preopen.

pub fn dump() {
    let mut coverage = Vec::new();
    // SAFETY: single-threaded guest; capture_coverage reads the instrumented counters once at run() end.
    unsafe {
        minicov::capture_coverage(&mut coverage).unwrap();
    }
    fs_err::write("/cov/et_ws_wasi_math1.profraw", coverage).unwrap();
}
