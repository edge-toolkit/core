//! Covers how the wasmtime bindings generator joins string literals written with line continuations.
#![cfg(test)]

use et_int_gen::wit::bindings::join_string_continuations;
use proc_macro2::TokenStream;

/// The tokens `source` parses to, after joining, printed back as text.
fn joined(source: &str) -> String {
    join_string_continuations(source.parse::<TokenStream>().unwrap()).to_string()
}

#[test]
fn a_continued_string_inside_a_macro_call_becomes_one_line() {
    let source = "fn f() { format_err!(\"does \\\n        not have `{name}`\") }";
    assert_eq!(joined(source), "fn f () { format_err ! (\"does not have `{name}`\") }");
}

#[test]
fn literals_without_a_continuation_are_left_as_written() {
    let source = r#"fn f() { g("plain\n", r"raw \ text", b"bytes", 42, 'c') }"#;
    assert_eq!(joined(source), source.parse::<TokenStream>().unwrap().to_string());
}

#[test]
fn a_continuation_in_a_byte_string_is_left_alone() {
    let source = "const B: &[u8] = b\"one \\\n    two\";";
    assert_eq!(joined(source), source.parse::<TokenStream>().unwrap().to_string());
}
