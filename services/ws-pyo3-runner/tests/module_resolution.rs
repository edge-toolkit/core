//! Covers the probe that decides where the runner gets its module from.
//!
//! `module_is_importable` is the whole of that decision: a name it finds on `sys.path` is imported the way it
//! always was, and only a name it does not find is fetched from the hub. Both answers matter, and the false one
//! is the interesting case -- the hub publishes modules under package names like `et-ws-pyo3-math1`, which is
//! not a legal Python identifier, so `find_spec` raises rather than returning `None` and the probe has to read
//! that as "not here" instead of propagating it.
#![cfg(test)]

use et_ws_pyo3_runner::python::module_is_importable;

#[test]
fn a_stdlib_module_is_importable_with_no_extra_paths() {
    assert!(
        module_is_importable("json", &[]).unwrap(),
        "the stdlib is always on sys.path"
    );
}

#[test]
fn a_hub_package_name_is_not_importable() {
    // Hyphens make this un-importable by construction, which is what sends the runner to the hub for it.
    assert!(
        !module_is_importable("et-ws-pyo3-math1", &[]).unwrap(),
        "a hub package name is not a Python identifier and must not be reported as importable"
    );
}

#[test]
fn an_unknown_module_is_not_importable() {
    assert!(
        !module_is_importable("et_no_such_module_anywhere", &[]).unwrap(),
        "a well-formed name that exists nowhere must still be reported as absent"
    );
}

#[test]
fn an_extra_path_makes_its_modules_importable() {
    // The non-empty branch: `echo` is only findable once PYO3_PYTHONPATH's directory is on sys.path, so this
    // also proves the probe searches the same paths the import will.
    let python_dir = edge_toolkit::config::get_project_root().join("services/ws-pyo3-runner/python");
    assert!(
        !module_is_importable("echo", &[]).unwrap(),
        "echo must not be importable before its directory is added"
    );
    assert!(
        module_is_importable("echo", &[python_dir]).unwrap(),
        "echo must be importable once its directory is added"
    );
}
