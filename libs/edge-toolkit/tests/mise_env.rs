//! Covers every arm of `mise_env_includes`, the gate each guest-language test asks before it runs.
//!
//! The function decides whether a language's toolchain is loaded, and every caller treats a `false` as
//! "skip this work". That makes both its failure modes silent: a wrong `true` runs a test against a toolchain
//! that is not installed, and a wrong `false` turns a real test into a no-op that still reports as passing.
//! The unset case in particular has to answer `true` -- a bare `cargo test` sets no `MISE_ENV`, and answering
//! `false` there would quietly disable the guest-language suites for anyone not going through a task.
#![cfg(test)]

use edge_toolkit::config::{Language, mise_env_includes};
use et_test_helpers::temp_env;

#[test]
fn an_unset_mise_env_includes_every_language() {
    // No MISE_ENV at all: the read fails and the answer is an unconditional `true`, so a plain `cargo test`
    // outside the task runner exercises every language rather than skipping them all.
    temp_env::with_var_unset("MISE_ENV", || {
        assert!(mise_env_includes(Language::Python));
        assert!(mise_env_includes(Language::Zig));
    });
}

#[test]
fn an_empty_mise_env_includes_nothing() {
    // An empty value is a set-but-empty env list, which is not the same as unset: nothing is loaded, so every
    // language must answer `false`. Splitting `""` on `,` yields one empty segment, which would match no
    // language anyway -- the explicit emptiness check is what keeps that from depending on split's behaviour.
    temp_env::with_var("MISE_ENV", Some(""), || {
        assert!(!mise_env_includes(Language::Python));
        assert!(!mise_env_includes(Language::Rust));
    });
}

#[test]
fn a_populated_mise_env_includes_only_what_it_lists() {
    // The everyday case, taking both arms of the membership test in one pass: a language in the list and one
    // absent from it. Segments are trimmed, so a list written with spaces resolves the same way.
    temp_env::with_var("MISE_ENV", Some("rust, python ,zig"), || {
        assert!(mise_env_includes(Language::Python));
        assert!(mise_env_includes(Language::Rust));
        assert!(mise_env_includes(Language::Zig));
        assert!(!mise_env_includes(Language::Java));
        assert!(!mise_env_includes(Language::Dart));
    });
}

#[test]
fn a_prefix_of_a_language_name_is_not_a_match() {
    // Segment equality, not substring: `r` must not satisfy `rust`, and `java` must not satisfy `js`.
    // A substring test here would silently enable suites whose toolchain is absent.
    temp_env::with_var("MISE_ENV", Some("r,java"), || {
        assert!(mise_env_includes(Language::R));
        assert!(mise_env_includes(Language::Java));
        assert!(!mise_env_includes(Language::Rust));
        assert!(!mise_env_includes(Language::Js));
    });
}
