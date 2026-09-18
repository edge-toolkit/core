//! Covers the two config-shaped decisions `init` makes before it touches any global state.
//!
//! Both live in `init`, which installs a process-global subscriber and so runs at most once per process.
//! Whichever configuration that one call happens to use is the only one a test could otherwise observe, and
//! the alternatives fail quietly: an exporter built without credentials is rejected by the collector on every
//! export, and a missing `service.instance` makes a service's telemetry indistinguishable between hosts.
#![cfg(test)]

use edge_toolkit::auth::BasicAuth;
use edge_toolkit::config::get_project_root;
use et_otlp::{exporter_headers, service_descriptors};
use fs_err as fs;
use opentelemetry::Value;

#[test]
fn headers_carry_basic_auth_only_when_the_config_supplies_it() {
    // No auth: the exporters go out bare, which is what a local collector expects.
    assert!(
        exporter_headers(None).is_empty(),
        "an unauthenticated config must add no headers at all"
    );

    // The credential comes from the committed dev-only env file the local collector starts from, so the
    // header is built from the value that collector actually accepts and a rotation there needs no matching
    // edit here. Read the way docker's `--env-file` reads it: everything after the `=` is the value, verbatim.
    let env_file = get_project_root().join("config/o2.env");
    let env_text = fs::read_to_string(&env_file).unwrap();
    let env_value = |key: &str| -> String {
        env_text
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_else(|| panic!("no {key} in {}", env_file.display()))
            .to_string()
    };
    let user = env_value("ZO_ROOT_USER_EMAIL");
    let password = env_value("ZO_ROOT_USER_PASSWORD");

    // With auth: exactly one `authorization` header, and the value is the base64 of `user:password`
    // rather than either half in the clear.
    let auth = BasicAuth::new(user, password.clone().into());
    let headers = exporter_headers(Some(&auth));
    let authorization = &headers["authorization"];
    assert!(
        authorization.starts_with("Basic "),
        "expected an HTTP basic credential, got {authorization:?}"
    );
    assert!(
        !authorization.contains(&password),
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
