//! Layout test for `find_site_packages_in` -- the pure-filesystem core of
//! `mise_python_site_packages`. Builds a tempdir mimicking the pipx venv layout
//! and verifies the resolver finds it on both POSIX
//! (`<install>/<pkg>/lib/python<X.Y>/site-packages`) and Windows
//! (`<install>/<pkg>/Lib/site-packages`, no Python-version subdir).

#![cfg(test)]

use command_error::CommandExt as _;
use edge_toolkit::config::find_site_packages_in;
use fs_err as fs;
use tempfile::TempDir;

/// Build `<install>/<venv>` and assert the resolver returns exactly that directory.
///
/// The two layouts below differ only in the directory names between the install root and `site-packages`,
/// which is the whole point of the resolver: it scans the variable segments rather than assuming either
/// shape. Asserting them through one body keeps that the only difference on display.
fn resolves_venv_layout(venv: &str) {
    let install = TempDir::new().unwrap();
    let site_packages = install.path().join(venv);
    fs::create_dir_all(&site_packages).unwrap();

    let found = find_site_packages_in(install.path());
    assert_eq!(found.as_deref(), Some(site_packages.as_path()));
}

#[test]
fn resolves_pipx_venv_layout() {
    // The shape mise's pipx backend lays down; both `<pkg>` and the python version are scanned, not assumed.
    resolves_venv_layout("cowsay/lib/python3.13/site-packages");
}

#[test]
fn resolves_windows_pipx_venv_layout() {
    // The shape uv (pipx backend) lays down on Windows: capital `Lib`, no python-version subdir.
    resolves_venv_layout("cowsay/Lib/site-packages");
}

/// Probe availability and run the lookup under whatever `PATH` the caller has arranged, reporting both.
///
/// Both are read under one `PATH` so the pair can be compared: the lookup answers with an empty list for two
/// quite different reasons -- mise was never found, or mise was found and its query failed -- and only the
/// availability flag distinguishes them. Asserting the list alone would let a test pass while exercising the
/// wrong path entirely.
fn availability_and_lookup() -> (bool, Vec<std::path::PathBuf>) {
    (
        edge_toolkit::config::mise_is_available(),
        edge_toolkit::config::mise_python_site_packages(),
    )
}

#[test]
fn site_packages_lookup_is_empty_when_mise_is_missing() {
    // `mise_python_site_packages` is what pre-populates the pyo3 runner's `sys.path`, and it must degrade to
    // an empty list rather than panicking when there is no mise to ask. An empty PATH makes the availability
    // probe's spawn fail the same way it would on a deployment that never installed mise, so the function
    // returns before it tries to parse any tool list.
    let (available, found) = et_test_helpers::with_empty_path(availability_and_lookup);
    assert!(!available, "an empty PATH must hide mise from the availability probe");
    assert!(
        found.is_empty(),
        "expected no site-packages paths when mise is unavailable, got {found:?}"
    );
}

/// Build a stand-in `mise` into `dir` that answers `--version` and fails every other invocation.
///
/// The availability probe and the tool-list call are two separate spawns of the same name, so the only way to
/// reach the "mise is here but the query failed" path is a command that distinguishes between them.
///
/// A real executable, compiled here by `rustc`, rather than a script: the probe spawns the bare name `mise`,
/// which Windows resolves to `mise.exe` and nothing else, so a `mise.bat` on the same `PATH` is never found
/// and the probe reports mise absent -- the very branch this fixture exists to get past. That is how the
/// windows-11-arm lane failed on commit
/// <https://github.com/edge-toolkit/core/commit/aec4133097f57ad406fb16ad5231fafb31641e09> with
/// `the stand-in must answer --version, or this asserts the wrong branch`
/// (<https://github.com/edge-toolkit/core/actions/runs/35042685720/job/104625726124>). Compiling costs a second
/// and needs `rustc` on `PATH`, which anything running `cargo test` already has; the one shape then serves
/// every platform, with no shell or batch dialect to keep in step.
#[expect(
    clippy::single_call_fn,
    reason = "distinct fixture builder for the stand-in command; kept separate from the assertions it feeds"
)]
fn write_fake_mise_that_fails_its_queries(dir: &TempDir) {
    const STAND_IN: &str = r#"fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("2026.1.1");
    } else {
        std::process::exit(1);
    }
}
"#;
    let source = dir.path().join("mise.rs");
    fs::write(&source, STAND_IN).unwrap();
    let exe = dir.path().join(format!("mise{}", std::env::consts::EXE_SUFFIX));
    let _compiled: std::process::Output = std::process::Command::new("rustc")
        .args(["--edition", "2021", "-o"])
        .arg(&exe)
        .arg(&source)
        .output_checked()
        .unwrap();
    // Run it once by path before anything depends on it, so a stand-in that compiled but cannot start fails
    // here with its own error instead of surfacing later as "mise is absent".
    let _probed: std::process::Output = std::process::Command::new(&exe)
        .arg("--version")
        .output_checked()
        .unwrap();
}

#[test]
fn a_failing_tool_list_query_yields_no_paths() {
    // mise resolves and reports a version, so the availability probe passes, but the tool-list query exits
    // non-zero -- a broken config, an unreadable install directory, a version whose `ls` flags differ. The
    // runner has to treat that as "no mise-managed packages" and carry on, because the alternative is an
    // embedded interpreter that refuses to start on a machine where everything else works.
    let dir = TempDir::new().unwrap();
    write_fake_mise_that_fails_its_queries(&dir);

    let (available, found) = et_test_helpers::with_path_prefix(dir.path(), availability_and_lookup);

    // Asserted first, and separately: an empty list is also what a *missing* mise produces, so without
    // pinning the stand-in as present this test would still pass if it were never found at all -- exercising
    // the earlier bail-out and reporting success for a path it never reached.
    assert!(
        available,
        "the stand-in must answer --version, or this asserts the wrong branch"
    );
    assert!(
        found.is_empty(),
        "a failing `mise ls` must yield no paths, got {found:?}"
    );
}

#[test]
fn a_tool_list_that_is_not_json_yields_no_paths() {
    // `mise ls --current --json` answered with something unparsable -- an older mise, a wrapper that printed
    // a warning first, a truncated pipe. The runner must come away with an empty `sys.path` addition rather
    // than failing to start, so the interpreter still boots and only the mise-managed imports are missing.
    assert!(edge_toolkit::config::site_packages_from_tool_list(b"mise: not a tool list").is_empty());
    assert!(
        edge_toolkit::config::site_packages_from_tool_list(b"").is_empty(),
        "an empty body is not valid JSON either"
    );
}

#[test]
fn only_pipx_tools_with_a_venv_contribute_paths() {
    // One `pipx:` tool whose install really carries a venv, one `pipx:` tool whose install does not, and one
    // non-`pipx:` tool. Only the first contributes: the others are what a mixed tool set looks like, and
    // letting either through would put a directory with no `site-packages` on the interpreter's path.
    let install = TempDir::new().unwrap();
    let with_venv = install.path().join("cowsay-install");
    let site_packages = with_venv.join("cowsay/lib/python3.13/site-packages");
    fs::create_dir_all(&site_packages).unwrap();
    let without_venv = install.path().join("bare-install");
    fs::create_dir_all(&without_venv).unwrap();

    let tool_list = serde_json::json!({
        "pipx:cowsay": [{ "active": true, "install_path": with_venv }],
        "pipx:bare": [{ "active": true, "install_path": without_venv }],
        "npm:some-node-tool": [{ "active": true, "install_path": with_venv }],
    });
    let found = edge_toolkit::config::site_packages_from_tool_list(&serde_json::to_vec(&tool_list).unwrap());
    assert_eq!(
        found,
        vec![site_packages],
        "expected only the pipx install that has a venv"
    );
}

#[test]
fn ignores_non_python_lib_dirs() {
    // A `lib/` whose only child isn't a `python*` dir must not match.
    let install = TempDir::new().unwrap();
    fs::create_dir_all(install.path().join("tool/lib/node")).unwrap();

    assert!(find_site_packages_in(install.path()).is_none());
}

#[test]
fn returns_none_without_site_packages() {
    // A `python*` dir exists but has no `site-packages` under it.
    let install = TempDir::new().unwrap();
    fs::create_dir_all(install.path().join("tool/lib/python3.13")).unwrap();

    assert!(find_site_packages_in(install.path()).is_none());
}
