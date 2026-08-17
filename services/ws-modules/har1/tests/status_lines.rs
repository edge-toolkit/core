//! Host-side check of the pure status-panel section helper (the wasm interop stays untested here).

#![cfg(test)]

use et_ws_har1::push_section;

#[test]
fn appends_a_blank_separator_then_the_title() {
    let mut lines = vec![String::from("header")];
    push_section(&mut lines, "orientation");
    push_section(&mut lines, "motion");
    assert_eq!(lines, ["header", "", "orientation", "", "motion"]);
}
