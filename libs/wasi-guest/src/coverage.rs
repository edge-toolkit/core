//! Coverage dump for the instrumented guest builds, isolated into its own file.
//!
//! minicov's `capture_coverage` is an `unsafe fn` (it reads the raw instrumented counter buffers), which
//! Codacy flags for audit. Codacy can only exclude whole paths in-repo, not suppress per line, so this
//! one-function file is the single thing excluded from Codacy (see .codacy.yaml) while the rest of this crate
//! and every guest that depends on it stays analyzed. The unsafe is still covered by the repo's own clippy and
//! by DeepSource. Each guest calls this at the end of `run()`, naming the profile its own build should write.

/// Dump the calling guest's coverage profile to the runner's `/cov` preopen, under `profraw_name`.
#[expect(
    unsafe_code,
    reason = "minicov::capture_coverage is an unsafe fn; reading the counters is this function's whole purpose"
)]
pub fn dump_coverage(profraw_name: &str) {
    let mut coverage = Vec::new();
    // SAFETY: single-threaded guest; capture_coverage reads the instrumented counters once at run() end.
    unsafe {
        minicov::capture_coverage(&mut coverage).unwrap();
    }
    fs_err::write(format!("/cov/{profraw_name}"), coverage).unwrap();
}
