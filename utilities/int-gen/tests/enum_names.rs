//! Covers how an enum whose JSON values cannot name a case in WIT or Dart is still emitted for both.
#![cfg(test)]

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_int_gen::kdl::{dart_variant_name, dart_variant_names, is_dart_identifier};
use et_int_gen::wit::messages::{are_faithful_wit_cases, is_wit_identifier};
use schemars::schema_for;

#[test]
fn wit_cases_that_collide_are_not_faithful() {
    let cases = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect::<Vec<_>>();
    assert!(are_faithful_wit_cases(&cases(&["assigned", "reconnected"])));
    // `foo_bar` and `foo-bar` both kebab-case to `foo-bar`, so one case could not stand for both values.
    assert!(!are_faithful_wit_cases(&cases(&["foo-bar", "foo-bar"])));
    assert!(!are_faithful_wit_cases(&cases(&["1-0"])));
}

#[test]
fn dart_members_that_collide_are_an_error_naming_both_values() {
    assert_eq!(
        dart_variant_names("Status", &["assigned", "1.0"]).unwrap(),
        ["assigned", "v1_0"]
    );
    let message = dart_variant_names("Version", &["1.0", "1-0"]).unwrap_err().to_string();
    assert_eq!(
        message,
        "enum `Version`: values `1.0` and `1-0` both name the Dart member `v1_0`"
    );
}

#[test]
fn wit_identifiers_are_words_that_each_open_with_a_letter() {
    assert!(is_wit_identifier("assigned"));
    assert!(is_wit_identifier("spec-version"));
    assert!(is_wit_identifier("v1"));
    assert!(!is_wit_identifier("1-0"));
    assert!(!is_wit_identifier("v1-0"));
    assert!(!is_wit_identifier("Assigned"));
    assert!(!is_wit_identifier(""));
}

#[test]
fn a_value_that_cannot_name_a_dart_member_gets_a_derived_name() {
    assert!(is_dart_identifier("assigned"));
    assert!(is_dart_identifier("_private"));
    assert!(!is_dart_identifier("1.0"));
    assert!(!is_dart_identifier(""));
    assert_eq!(dart_variant_name("assigned"), "assigned");
    assert_eq!(dart_variant_name("1.0"), "v1_0");
}

#[test]
fn the_spec_version_enum_is_a_string_alias_in_wit_while_other_enums_stay_enums() {
    let wit = et_int_gen::wit::messages::render(&schema_for!(ClientMessage), &schema_for!(ServerMessage)).unwrap();
    assert!(wit.contains("type spec-version = string;"));
    assert!(wit.contains("enum connect-status {"));
}

#[test]
fn the_spec_version_enum_keeps_its_wire_value_in_kdl_while_other_variants_stay_bare() {
    let kdl = et_int_gen::kdl::render(&schema_for!(ClientMessage), &schema_for!(ServerMessage)).unwrap();
    assert!(kdl.contains("variant \"v1_0\" {\n        json-value \"1.0\"\n    }"));
    assert!(kdl.contains("variant \"assigned\"\n"));
}
