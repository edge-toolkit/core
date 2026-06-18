//! Verifies the `http:pyodide` mise install carries the *full* release
//! distribution -- not just the runtime (`pyodide.asm.{js,wasm}` +
//! `python_stdlib.zip`). This is the difference between the ~200 MB
//! GitHub release tarball (~300 wheels: numpy, scipy, pandas, ...) and
//! `npm:pyodide`, which ships only the runtime. ws-server's modules
//! service prefers `http:pyodide` precisely so that browser modules
//! calling `micropip.install("numpy")` can resolve the wheel offline.
//!
//! The test runs against the live mise install -- if `http:pyodide` is
//! missing, the test fails with a `mise install` hint rather than
//! silently passing.

#![cfg(test)]
#![expect(
    clippy::panic,
    clippy::print_stderr,
    clippy::unwrap_used,
    reason = "test code: missing install fails loudly with a hint; skip-path logs to stderr"
)]

use std::collections::HashSet;
use std::path::PathBuf;

use edge_toolkit::config::{Language, default_modules_folders, mise_env_includes, mise_where};

/// Shared resolver -- every test in this file wants the install path.
/// Returns `None` when `http:pyodide` is intentionally not installed
/// because `MISE_ENV` doesn't include the `python` env; callers early-return
/// in that case (the test is meaningless without pyodide installed).
/// Otherwise (no `MISE_ENV`, or `MISE_ENV` includes `python`) panics with
/// a `mise install` hint so a broken install is surfaced loudly.
fn require_http_pyodide_install() -> Option<PathBuf> {
    if let Some(path) = mise_where("http:pyodide") {
        return Some(path);
    }
    if !mise_env_includes(Language::Python) {
        eprintln!("skipping: http:pyodide not installed (MISE_ENV omits `python`)");
        return None;
    }
    panic!(
        "{}",
        concat!(
            "`mise where http:pyodide` returned no install path. ",
            "Run `mise install http:pyodide` first (~200 MB download). ",
            "The `npm:pyodide` fallback only carries the runtime, not the wheels.",
        )
    )
}

/// Lower bound -- the official 0.29.x release ships well over 300 wheels.
/// 100 is conservative enough to survive minor releases dropping a few
/// rarely-used packages without flapping in CI.
const MIN_WHEEL_COUNT: usize = 100;

/// Wheels we always expect in the full distribution. Each entry is a
/// `<package>-` prefix; the test passes if at least one filename in
/// the install starts with it. These three are the "is this the full
/// release?" canaries: none of them ship with `npm:pyodide`.
const REQUIRED_WHEEL_PREFIXES: &[&str] = &["numpy-", "scipy-", "pandas-"];

#[test]
fn http_pyodide_install_contains_full_wheel_set() {
    let Some(install) = require_http_pyodide_install() else {
        return;
    };

    let entries = fs_err::read_dir(&install).unwrap();

    let wheel_names: HashSet<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "whl"))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();

    assert!(
        wheel_names.len() >= MIN_WHEEL_COUNT,
        "expected >={MIN_WHEEL_COUNT} wheels at {}, found {}. Runtime-only install?",
        install.display(),
        wheel_names.len(),
    );

    for prefix in REQUIRED_WHEEL_PREFIXES {
        assert!(
            wheel_names.iter().any(|name| name.starts_with(prefix)),
            "missing wheel starting with `{prefix}` in {}. Found: {wheel_names:?}",
            install.display(),
        );
    }
}

#[test]
fn http_pyodide_install_has_runtime_too() {
    // The runtime files live next to the wheels in the same flat dir.
    // ws-server's static-file serve relies on this -- guests fetch
    // `/modules/pyodide/pyodide.asm.wasm` from the same prefix as
    // `/modules/pyodide/numpy-*.whl`.
    let Some(install) = require_http_pyodide_install() else {
        return;
    };

    for runtime_file in [
        "package.json",
        "pyodide.asm.wasm",
        "pyodide.asm.js",
        "python_stdlib.zip",
    ] {
        let path = install.join(runtime_file);
        assert!(
            path.is_file(),
            "{} missing from http:pyodide install -- expected at {}",
            runtime_file,
            path.display(),
        );
    }
}

#[test]
fn default_modules_folders_prefers_http_pyodide() {
    // When `http:pyodide` is present, `default_modules_folders` returns
    // the http install dir (one directory, treated as a single module
    // named "pyodide") rather than the npm `node_modules` parent dir.
    // This pins the resolver behaviour so a future refactor that
    // accidentally reorders the fallback gets caught.
    let Some(http_install) = require_http_pyodide_install() else {
        return;
    };

    let paths = default_modules_folders();
    assert!(
        paths.contains(&http_install),
        "default_modules_folders() did not include http:pyodide dir {}. Got: {paths:?}",
        http_install.display(),
    );
}
