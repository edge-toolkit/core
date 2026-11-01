//! Covers what a `store.open` identifier opens on every host, and the `store.error` values a bucket call fails with.
#![cfg(test)]

use et_ws_wasi_runner::bindings::wasi::keyvalue::store::Error;
use et_ws_wasi_runner::keyvalue::{Bucket, bucket_from_identifier, kv_not_implemented};
use rstest::rstest;
use wasmtime::component::ResourceTableError;

#[rstest]
#[case::agent("3f2c9a6e-agent", "3f2c9a6e-agent")]
#[case::agent_named_like_a_module("modules", "modules")]
fn an_agent_id_opens_its_storage_bucket(#[case] identifier: &str, #[case] expected: &str) {
    let bucket = bucket_from_identifier(identifier).unwrap();
    assert!(matches!(bucket, Bucket::Storage { agent_id } if agent_id == expected));
}

#[rstest]
#[case::plain("modules/wasi-math1", "wasi-math1")]
#[case::scoped("modules/@edge-toolkit/wasi-math1", "@edge-toolkit/wasi-math1")]
fn a_module_name_opens_its_asset_bucket(#[case] identifier: &str, #[case] expected: &str) {
    let bucket = bucket_from_identifier(identifier).unwrap();
    assert!(matches!(bucket, Bucket::Modules { module_name } if module_name == expected));
}

#[rstest]
#[case::empty("")]
#[case::agent_path("agent/file.json")]
#[case::no_module("modules/")]
#[case::module_path("modules/wasi-math1/pkg")]
#[case::scope_only("modules/@edge-toolkit")]
#[case::empty_scope("modules/@/wasi-math1")]
#[case::empty_scoped_name("modules/@edge-toolkit/")]
#[case::scoped_module_path("modules/@edge-toolkit/wasi-math1/pkg")]
fn an_identifier_naming_a_path_opens_nothing(#[case] identifier: &str) {
    let error = bucket_from_identifier(identifier).err().unwrap();
    assert!(matches!(error, Error::Other(detail) if detail.contains(&format!("{identifier:?}"))));
}

#[test]
fn an_unimplemented_call_names_its_operation() {
    assert!(matches!(kv_not_implemented("list-keys"), Error::Other(detail) if detail == "list-keys not implemented"));
}

#[test]
fn a_stale_bucket_handle_is_reported_as_the_table_explains_it() {
    let error = Error::from(ResourceTableError::NotPresent);
    assert!(matches!(error, Error::Other(detail) if detail == ResourceTableError::NotPresent.to_string()));
}
