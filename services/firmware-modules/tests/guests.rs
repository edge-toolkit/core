//! Covers the `guests.rs` source a firmware build writes: each runner's component found among the modules built in,
//! checked against what a board hosts and holds, and compiled for it, and every way a guest is refused.
#![cfg(test)]

use std::path::{Path, PathBuf};

use edge_toolkit::config::get_project_root;
use et_firmware_modules::{Generated, ModulesError, guests};
use fs_err as fs;
use tempfile::{TempDir, tempdir};

/// The Pulley target a 64-bit host's board engine runs, which the tests compile for.
const TARGET: &str = "pulley64";
/// More memory than any guest here starts with.
const ROOMY: u64 = 64 * 1024 * 1024;
/// One WebAssembly page, 64 KiB.
const PAGE: u64 = 64 * 1024;
/// Two pages, what the guest declaring `(memory 2)` starts with.
const TWO_PAGES: u64 = 2 * PAGE;
/// A page, and a four-element table on a 64-bit board.
const PAGE_AND_FOUR_POINTERS_64: u64 = PAGE + 4 * 8;
/// A page, and a four-element table on a 32-bit board.
const PAGE_AND_FOUR_POINTERS_32: u64 = PAGE + 4 * 4;

/// A module in `root` named `name`, its `package.json` naming `main`, with `component` written there.
fn module(root: &Path, name: &str, main: &str, component: &[u8]) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("package.json"),
        format!(r#"{{"name": "{name}", "main": "{main}"}}"#),
    )
    .unwrap();
    fs::write(dir.join(main), component).unwrap();
    dir
}

/// A module holding the component `text` describes, in WebAssembly text, as its `main`.
fn component_module(text: &str) -> (TempDir, String) {
    let root = tempdir().unwrap();
    let dir = module(root.path(), "guest", "guest.wasm", &wat::parse_str(text).unwrap());
    (root, dir.display().to_string())
}

/// What `guests` returns for one runner, `runner=guest`, of the module at `path`, holding `memory` for it.
fn one_guest(path: &str, memory: u64, out_dir: &Path) -> Result<Generated, ModulesError> {
    guests(path, "runner-1=guest", TARGET, memory, out_dir)
}

#[test]
fn no_runners_build_in_no_guests() {
    let out_dir = tempdir().unwrap();
    let generated = guests("", " , ", TARGET, 0, out_dir.path()).unwrap();
    assert!(
        generated
            .source
            .contains("pub(crate) const GUESTS: [(&str, &[u8]); 0] = [\n];")
    );
    assert_eq!(generated.files, Vec::<PathBuf>::new());
    assert!(generated.source.contains("pub(crate) const GUEST_MEMORY: u64 = 0;"));
}

#[test]
fn the_board_s_budget_is_written_for_its_guests_to_draw_on() {
    let out_dir = tempdir().unwrap();
    let generated = guests("", "", TARGET, ROOMY, out_dir.path()).unwrap();
    assert!(
        generated
            .source
            .contains("pub(crate) const GUEST_MEMORY: u64 = 67_108_864;")
    );
    let generated = guests("", "", TARGET, PAGE, out_dir.path()).unwrap();
    assert!(
        generated
            .source
            .contains("pub(crate) const GUEST_MEMORY: u64 = 65_536;")
    );
}

#[test]
fn a_runner_s_module_is_compiled_for_the_board_and_built_in() {
    let data1 = get_project_root().join("services/ws-modules/wasi-data1");
    let out_dir = tempdir().unwrap();
    let paths = format!("{}", data1.display());
    let generated = guests(
        &paths,
        "data1 = @edge-toolkit/et-ws-wasi-data1",
        TARGET,
        ROOMY,
        out_dir.path(),
    )
    .unwrap();
    let compiled = out_dir.path().join("guest-0.cwasm");
    assert!(compiled.is_file());
    assert!(generated.source.contains("[(&str, &[u8]); 1]"));
    assert!(generated.source.contains(&format!(
        "(\"data1\", include_bytes!({:?})),",
        compiled.display().to_string()
    )));
    assert_eq!(generated.files, [data1.join("pkg/et_ws_wasi_data1.wasm")]);
}

#[test]
fn a_runner_that_is_not_a_pair_is_refused() {
    let out_dir = tempdir().unwrap();
    for pair in ["no-package", "=guest", "runner-1= "] {
        let pair = pair.trim();
        let refused = guests("", pair, TARGET, ROOMY, out_dir.path()).unwrap_err();
        assert!(matches!(refused, ModulesError::NotAGuest(held) if held == pair));
    }
}

#[test]
fn a_runner_of_a_module_not_built_in_is_refused() {
    let (_root, path) = component_module("(component)");
    let out_dir = tempdir().unwrap();
    let refused = guests(&path, "runner-1=elsewhere", TARGET, ROOMY, out_dir.path()).unwrap_err();
    assert!(matches!(
        refused,
        ModulesError::UnknownGuest { runner, package, names }
            if runner == "runner-1" && package == "elsewhere" && names == ["guest"]
    ));
}

#[test]
fn a_runner_of_a_module_with_no_component_is_refused() {
    let root = tempdir().unwrap();
    let dir = module(root.path(), "guest", "guest.js", b"export {};");
    let out_dir = tempdir().unwrap();
    let refused = one_guest(&dir.display().to_string(), ROOMY, out_dir.path()).unwrap_err();
    assert!(matches!(refused, ModulesError::NotAComponent { main, .. } if main == "guest.js"));
}

#[test]
fn a_component_that_is_not_webassembly_is_refused() {
    let root = tempdir().unwrap();
    let dir = module(root.path(), "guest", "guest.wasm", b"not wasm");
    let out_dir = tempdir().unwrap();
    let refused = one_guest(&dir.display().to_string(), ROOMY, out_dir.path()).unwrap_err();
    assert!(matches!(refused, ModulesError::Unreadable { path, .. } if path == dir.join("guest.wasm")));
}

#[test]
fn a_guest_importing_what_the_board_does_not_host_is_refused() {
    let (_root, path) = component_module(
        r#"(component
            (import "wasi:io/poll@0.2.12" (instance))
            (import "wasi:webgpu/webgpu@0.3.0-rc.2" (instance))
        )"#,
    );
    let out_dir = tempdir().unwrap();
    let refused = one_guest(&path, ROOMY, out_dir.path()).unwrap_err();
    let webgpu = "wasi:webgpu/webgpu@0.3.0-rc.2";
    assert!(matches!(
        refused,
        ModulesError::Unhosted { runner, imports } if runner == "runner-1" && imports == [webgpu]
    ));
}

#[test]
fn guests_needing_more_memory_than_the_board_holds_are_refused() {
    let (_root, path) = component_module("(component (core module (memory 2)))");
    let out_dir = tempdir().unwrap();
    let refused = one_guest(&path, PAGE, out_dir.path()).unwrap_err();
    assert!(matches!(
        refused,
        ModulesError::GuestMemory {
            needed: TWO_PAGES,
            budget: PAGE
        }
    ));
    let _fits = one_guest(&path, TWO_PAGES, out_dir.path()).unwrap();
}

#[test]
fn a_guest_s_tables_count_at_the_board_s_pointer_width() {
    let (_root, path) = component_module("(component (core module (memory 1) (table 4 funcref)))");
    let out_dir = tempdir().unwrap();
    for (target, needed) in [
        ("pulley64", PAGE_AND_FOUR_POINTERS_64),
        ("pulley32", PAGE_AND_FOUR_POINTERS_32),
    ] {
        let refused = guests(&path, "runner-1=guest", target, PAGE, out_dir.path()).unwrap_err();
        assert!(matches!(refused, ModulesError::GuestMemory { needed: held, .. } if held == needed));
        let _fits = guests(&path, "runner-1=guest", target, needed, out_dir.path()).unwrap();
    }
}

#[test]
fn a_runner_s_main_outside_its_package_is_refused() {
    let root = tempdir().unwrap();
    let dir = module(root.path(), "guest", "guest.wasm", b"\0asm");
    fs::write(
        dir.join("package.json"),
        r#"{"name": "guest", "main": "/elsewhere.wasm"}"#,
    )
    .unwrap();
    let out_dir = tempdir().unwrap();
    let refused = one_guest(&dir.display().to_string(), ROOMY, out_dir.path()).unwrap_err();
    assert!(matches!(refused, ModulesError::OutsidePackage { entry, .. } if entry == "/elsewhere.wasm"));
}

#[test]
fn a_guest_that_does_not_compile_for_the_target_is_refused() {
    let (_root, path) = component_module("(component)");
    let out_dir = tempdir().unwrap();
    let refused = guests(&path, "runner-1=guest", "no-such-target", ROOMY, out_dir.path()).unwrap_err();
    assert!(matches!(refused, ModulesError::Precompile { runner, .. } if runner == "runner-1"));
}

#[test]
fn a_relative_directory_to_compile_into_is_refused() {
    let refused = guests("", "", TARGET, ROOMY, Path::new("relative")).unwrap_err();
    assert!(matches!(refused, ModulesError::RelativePath { variable, .. } if variable == "RUNNER_GUESTS"));
}
