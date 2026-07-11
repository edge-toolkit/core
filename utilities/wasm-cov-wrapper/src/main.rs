//! `RUSTC_WORKSPACE_WRAPPER` for the browser-module wasm coverage build.
//!
//! cargo runs this as `int-wasm-cov-wrapper <rustc> <rustc-args...>` for **workspace** crates only (never registry
//! dependencies), so only our own crates are instrumented -- dependencies get neither coverage nor a broken
//! `-Cinstrument-coverage` link (a dependency cdylib such as tracing-wasm or wasm-streams would otherwise want an
//! unresolved `__llvm_profile_runtime`). Real crate compiles (those carrying `--crate-name`) get the wasm coverage
//! flags appended; cargo's `-vV` / `--print` probes pass through untouched. minicov (pulled by `et-web/coverage`)
//! supplies the profiler runtime for the instrumented crates, and the `--emit=llvm-ir` output feeds the wasm-cov
//! task that turns each module's coverage map into lcov.
//!
//! Excluded from Codacy analysis (`.codacy.yaml`): Codacy's Rust security rule flags `args_os()` flowing into a
//! subprocess spawn as a command-injection shape and cannot suppress it per line. That is a false positive here --
//! the args are cargo's own trusted rustc invocation, and forwarding argv to rustc is this file's entire purpose,
//! so no code change removes it. The file is kept minimal and single-purpose so the path exclude is as narrow as
//! possible; it stays fully covered by clippy and DeepSource's Rust analyzer.
#![expect(
    clippy::print_stderr,
    reason = "a build-tool wrapper reports its own startup failure to stderr"
)]

use std::ffi::OsString;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(rustc) = args.next() else {
        eprintln!("int-wasm-cov-wrapper: expected the rustc path as the first argument");
        return ExitCode::FAILURE;
    };
    let mut rustc_args: Vec<OsString> = args.collect();

    // Instrument only real crate compiles for the wasm target. `--crate-name` excludes cargo's `rustc -vV` /
    // `--print` probes; the `wasm32` target gate excludes host build scripts + proc-macros -- those compile for
    // the host, and instrumenting them just litters `.profraw` files in the module dir when they run at build
    // time (their coverage is irrelevant to the browser module anyway).
    if is_wasm_crate_compile(&rustc_args) {
        rustc_args.extend(
            [
                "--emit=llvm-ir",
                "-Cinstrument-coverage",
                "-Ccodegen-units=1",
                "-Clto=off",
                "-Zno-profiler-runtime",
            ]
            .into_iter()
            .map(OsString::from),
        );
    }

    match Command::new(&rustc).args(&rustc_args).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_nonzero) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("int-wasm-cov-wrapper: failed to run rustc: {error}");
            ExitCode::FAILURE
        }
    }
}

/// True when these rustc args are a real crate compile (`--crate-name`) targeting wasm32.
///
/// Both gates matter: `--crate-name` skips cargo's `-vV`/`--print` probes, and the wasm32 target skips host
/// build scripts + proc-macros (whose instrumented binaries would drop stray `.profraw` files at build time).
/// Handles both `--target wasm32-...` (two args) and `--target=wasm32-...` (one arg) spellings.
fn is_wasm_crate_compile(rustc_args: &[OsString]) -> bool {
    let is_crate = rustc_args.iter().any(|arg| arg.to_str() == Some("--crate-name"));
    let targets_wasm = rustc_args.iter().enumerate().any(|(index, arg)| {
        let Some(text) = arg.to_str() else { return false };
        text.strip_prefix("--target=")
            .is_some_and(|value| value.starts_with("wasm32"))
            || (text == "--target"
                && rustc_args
                    .get(index + 1)
                    .and_then(|next| next.to_str())
                    .is_some_and(|value| value.starts_with("wasm32")))
    });
    is_crate && targets_wasm
}
