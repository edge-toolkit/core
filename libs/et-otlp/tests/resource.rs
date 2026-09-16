//! Covers the two config-shaped decisions `init` makes before it touches any global state.
//!
//! Both live in `init`, which installs a process-global subscriber and so runs at most once per process.
//! Whichever configuration that one call happens to use is the only one a test could otherwise observe, and
//! the alternatives fail quietly: an exporter built without credentials is rejected by the collector on every
//! export, and a missing `service.instance` makes a service's telemetry indistinguishable between hosts.
#![cfg(test)]

use edge_toolkit::auth::BasicAuth;
use et_otlp::{exporter_headers, service_descriptors};
use opentelemetry::Value;

#[test]
fn headers_carry_basic_auth_only_when_the_config_supplies_it() {
    // No auth: the exporters go out bare, which is what a local collector expects.
    assert!(
        exporter_headers(None).is_empty(),
        "an unauthenticated config must add no headers at all"
    );

    // With auth: exactly one `authorization` header, and the value is the base64 of `user:password`
    // rather than either half in the clear.
    let auth = BasicAuth::new("root@example.com".to_string(), "Complexpass#123".to_string().into());
    let headers = exporter_headers(Some(&auth));
    let authorization = &headers["authorization"];
    assert!(
        authorization.starts_with("Basic "),
        "expected an HTTP basic credential, got {authorization:?}"
    );
    assert!(
        !authorization.contains("Complexpass#123"),
        "the password must be encoded, not passed through in the clear"
    );
    assert_eq!(headers.len(), 1, "no other headers should be added: {headers:?}");
}

#[test]
fn a_resolved_hostname_becomes_the_service_instance() {
    let descriptors = service_descriptors(Some("runner-7".to_string()));
    let instance = descriptors
        .iter()
        .find(|attr| attr.key.as_str() == "service.instance")
        .unwrap_or_else(|| panic!("a resolved hostname must be recorded as service.instance: {descriptors:?}"));
    assert_eq!(instance.value, Value::from("runner-7"));
    assert!(
        descriptors.iter().any(|attr| attr.key.as_str() == "service.version"),
        "the crate version is unconditional and must survive alongside the instance"
    );
}

#[test]
fn an_unresolvable_hostname_drops_the_instance_but_keeps_the_version() {
    // The host's name did not resolve, or was not UTF-8. The resource must still describe the service --
    // dropping the version here would leave the telemetry with nothing to identify the build by.
    let descriptors = service_descriptors(None);
    assert!(
        !descriptors.iter().any(|attr| attr.key.as_str() == "service.instance"),
        "an unresolvable hostname must not produce an empty service.instance: {descriptors:?}"
    );
    assert_eq!(descriptors.len(), 1, "only the version should remain: {descriptors:?}");
    assert_eq!(descriptors[0].key.as_str(), "service.version");
}
