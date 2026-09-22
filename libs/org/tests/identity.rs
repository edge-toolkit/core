//! The assembled identity strings, pinned to the exact text registries and manifests are matched against.
//!
//! Worth asserting despite being constants: each is built by `concat!` from one literal, so a missing
//! separator or a stray segment is a compile-time success and a runtime mismatch -- a scope without its
//! trailing slash still compiles, and then every package name built from it is wrong.
#![cfg(test)]

use et_org::{CRATE_PREFIX, IMAGE_REGISTRY, NPM_SCOPE, ORG, REPOSITORY_URL};

#[test]
fn the_identity_strings_are_what_registries_are_matched_against() {
    assert_eq!(ORG, "edge-toolkit");
    assert_eq!(CRATE_PREFIX, "et-");
    assert_eq!(NPM_SCOPE, "@edge-toolkit/");
    assert_eq!(REPOSITORY_URL, "https://github.com/edge-toolkit/core");
    assert_eq!(IMAGE_REGISTRY, "ghcr.io/edge-toolkit/core");
}

#[test]
fn every_assembled_string_carries_the_organisation_it_was_built_from() {
    // The point of assembling them: renaming the organisation has to move all of these together, and a
    // constant that stopped containing it would be one that had been written out by hand again.
    for assembled in [NPM_SCOPE, REPOSITORY_URL, IMAGE_REGISTRY] {
        assert!(assembled.contains(ORG), "{assembled} does not carry {ORG}");
    }
}

#[test]
fn the_scope_ends_with_its_separator_so_a_package_name_can_follow_it() {
    // `NPM_SCOPE` is concatenated directly onto a bare module name, so the separator has to be part of it.
    assert!(NPM_SCOPE.ends_with('/'));
    assert_eq!(format!("{NPM_SCOPE}et-ws-math1"), "@edge-toolkit/et-ws-math1");
}
