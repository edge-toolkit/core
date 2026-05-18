//! Build script: bake the active Python's lib directory in as an rpath so
//! the `et-ws-pyo3-runner` binary finds `libpython*.so` at runtime without
//! the operator having to set `LD_LIBRARY_PATH`.
//!
//! pyo3-build-config (a transitive dep via pyo3) inspects whatever
//! interpreter the build resolves (`PYO3_PYTHON`, then `python3` on
//! PATH) and exposes its `lib_dir` to us. We forward that as a linker
//! arg targeted only at the runner binary so the rlib half of the
//! crate is unaffected.

fn main() {
    let config = pyo3_build_config::get();
    let Some(lib_dir) = &config.lib_dir else {
        // pyo3 will already have emitted its own diagnostics about
        // which Python it picked up; nothing useful for us to add.
        return;
    };

    // -rpath is the runtime search path on ELF; macOS uses
    // @loader_path/<lib_dir> via -rpath too in modern linkers, so the
    // same arg covers both targets we care about.
    println!("cargo:rustc-link-arg-bin=et-ws-pyo3-runner=-Wl,-rpath,{lib_dir}");
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
}
