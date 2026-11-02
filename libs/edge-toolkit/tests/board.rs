//! What a board hosts, and what a guest component is read as asking of it.
#![cfg(test)]

use edge_toolkit::board::{COVERAGE_IMPORTS, Guest, HOSTED_IMPORTS, PREVIEW2, hosts};

/// The page size every memory here declares but the one that names its own, 64 KiB.
const PAGE: u64 = 64 * 1024;
/// What the guest below starts with: seventeen pages, and one memory of one byte-sized page.
const DECLARED: u64 = 17 * PAGE + 1;

/// The bytes of a component written in WebAssembly text.
fn component(text: &str) -> Vec<u8> {
    wat::parse_str(text).unwrap()
}

/// A component's bytes with the start of the first occurrence of `found` overwritten by `written`.
fn corrupted(bytes: &[u8], found: &[u8], written: &[u8]) -> Vec<u8> {
    let start = bytes.windows(found.len()).position(|window| window == found).unwrap();
    let mut corrupted = bytes.to_vec();
    corrupted
        .iter_mut()
        .skip(start)
        .zip(written)
        .for_each(|(byte, replacement)| *byte = *replacement);
    corrupted
}

#[test]
fn every_hosted_interface_is_hosted_at_the_version_it_is_linked_at() {
    for import in HOSTED_IMPORTS {
        assert!(hosts(import));
    }
    let wasi = HOSTED_IMPORTS.iter().filter(|import| {
        ["wasi:cli/", "wasi:clocks/", "wasi:io/"]
            .iter()
            .any(|package| import.starts_with(package))
    });
    for import in wasi {
        assert!(import.ends_with(&format!("@{PREVIEW2}")));
    }
}

#[test]
fn an_import_is_hosted_at_any_version_in_the_series_linked() {
    for import in ["wasi:io/poll@0.2.0", "wasi:io/poll@0.2.99", "et:ws-wasi/ws@0.2.7"] {
        assert!(hosts(import));
    }
    for import in [
        "wasi:io/poll",
        "wasi:io/poll@0.3.0",
        "wasi:io/poll@1.0.0",
        "wasi:io/poll@2",
        "wasi:io/poll@0.0.12",
        "wasi:logging/logging@0.1.1-draft",
        "wasi:logging/logging@0.1.0",
        "wasi:keyvalue/store@0.2.0-draft+build",
    ] {
        assert!(!hosts(import));
    }
}

#[test]
fn an_interface_only_a_host_runner_has_is_not_hosted() {
    assert!(!hosts("wasi:webgpu/webgpu@0.3.0-rc.2"));
    assert!(!hosts("wasi:nn/graph@0.2.0-rc-2024-10-28"));
    assert!(!hosts("wasi:random/random@0.2.12"));
}

#[test]
fn what_an_instrumented_guest_writes_its_profile_through_is_hosted_only_by_a_coverage_build() {
    for import in COVERAGE_IMPORTS {
        assert_eq!(hosts(import), cfg!(feature = "coverage"));
        assert!(import.ends_with(&format!("@{PREVIEW2}")));
    }
}

#[test]
fn a_guest_is_read_as_its_imports_and_its_memory() {
    let guest = Guest::read(&component(
        r#"(component
            (import "wasi:io/poll@0.2.12" (instance))
            (import "wasi:webgpu/webgpu@0.3.0-rc.2" (instance))
            (core module (memory 17) (table 3 funcref))
            (core module
                (import "env" "shared" (memory 3))
                (import "env" "table" (table 9 funcref))
                (memory 1 (pagesize 1))
                (table 2 externref)
            )
        )"#,
    ))
    .unwrap();
    assert_eq!(guest.imports, ["wasi:io/poll@0.2.12", "wasi:webgpu/webgpu@0.3.0-rc.2"]);
    assert_eq!(guest.memory, DECLARED);
    assert_eq!(guest.table_elements, 5);
    assert_eq!(guest.starts_with(4), DECLARED + 20);
    assert_eq!(guest.starts_with(8), DECLARED + 40);
    assert_eq!(guest.unhosted().collect::<Vec<_>>(), ["wasi:webgpu/webgpu@0.3.0-rc.2"]);
}

#[test]
fn a_nested_component_s_imports_are_its_own() {
    let guest = Guest::read(&component(
        r#"(component
            (component (import "wasi:nn/graph" (instance)))
            (import "wasi:logging/logging@0.1.0-draft" (instance))
        )"#,
    ))
    .unwrap();
    assert_eq!(guest.imports, ["wasi:logging/logging@0.1.0-draft"]);
    assert_eq!(guest.memory, 0);
    assert_eq!(guest.unhosted().count(), 0);
}

#[test]
fn what_is_not_webassembly_is_refused() {
    let _refused = Guest::read(b"not wasm").unwrap_err();
}

#[test]
fn an_unreadable_import_is_refused() {
    let bytes = component(r#"(component (import "wasi:io/poll" (instance)))"#);
    let name = b"wasi:io/poll";
    let length = u8::try_from(name.len()).unwrap();
    let mut from = vec![length];
    from.extend_from_slice(name);
    let _refused = Guest::read(&corrupted(&bytes, &from, &[0x7f])).unwrap_err();
}

#[test]
fn an_unreadable_memory_is_refused() {
    let bytes = component("(component (core module (memory 17)))");
    // The memory section: id 5, three bytes, one memory with no limits flags and 17 pages.
    let section = [0x05, 0x03, 0x01, 0x00, 0x11];
    // The same memory, its flags byte set to bits no memory may have.
    let unreadable = [0x05, 0x03, 0x01, 0x7f];
    let _refused = Guest::read(&corrupted(&bytes, &section, &unreadable)).unwrap_err();
}

#[test]
fn an_unreadable_table_is_refused() {
    let bytes = component("(component (core module (table 3 funcref)))");
    // The table section: id 4, four bytes, one funcref table with no limits flags and 3 elements.
    let section = [0x04, 0x04, 0x01, 0x70, 0x00, 0x03];
    // The same table, its element type a byte no reference type has.
    let unreadable = [0x04, 0x04, 0x01, 0x7f];
    let _refused = Guest::read(&corrupted(&bytes, &section, &unreadable)).unwrap_err();
}
