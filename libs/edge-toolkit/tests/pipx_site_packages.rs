//! Layout test for `find_site_packages_in` -- the pure-filesystem core of
//! `mise_python_site_packages`. Builds a tempdir mimicking the pipx venv layout
//! (`<install>/<pkg>/lib/python<X.Y>/site-packages`) and verifies the resolver
//! finds it regardless of the `<pkg>` name and python version.

#![cfg(test)]
#![expect(clippy::unwrap_used, reason = "test code: failed tempdir setup should fail the test")]

use edge_toolkit::config::find_site_packages_in;
use fs_err as fs;
use tempfile::TempDir;

#[test]
fn resolves_pipx_venv_layout() {
    // <install>/cowsay/lib/python3.13/site-packages -- the shape mise's pipx
    // backend lays down; both `<pkg>` and the python version are scanned, not
    // assumed.
    let install = TempDir::new().unwrap();
    let site_packages = install.path().join("cowsay/lib/python3.13/site-packages");
    fs::create_dir_all(&site_packages).unwrap();

    let found = find_site_packages_in(install.path());
    assert_eq!(found.as_deref(), Some(site_packages.as_path()));
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
