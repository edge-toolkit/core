//! Covers the TLS bootstrap helpers: self-signed generation, PEM round-trip load, and rustls config build.
#![cfg(test)]

use et_ws_server::tls::{build_tls_server_config, generate_tls_certs, load_tls_certs};
use tempfile::tempdir;

#[test]
fn generate_write_load_and_build_server_config() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");

    // Generate a self-signed pair, which also writes both PEM files to disk.
    let (gen_cert, gen_key) = generate_tls_certs(&cert, &key);
    assert!(
        cert.exists() && key.exists(),
        "generate_tls_certs must write both PEM files"
    );

    // The freshly-written PEMs must load back into the exact same der bytes that were generated.
    let (loaded_cert, loaded_key) = load_tls_certs(&cert, &key);
    assert_eq!(gen_cert, loaded_cert, "reloaded cert der must match the generated der");
    assert_eq!(
        gen_key.secret_der(),
        loaded_key.secret_der(),
        "reloaded key der must match the generated der"
    );

    let from_generated = build_tls_server_config(gen_cert, gen_key);
    assert!(
        from_generated.alpn_protocols.is_empty(),
        "no ALPN protocols are configured by default"
    );

    let from_loaded = build_tls_server_config(loaded_cert, loaded_key);
    assert!(
        from_loaded.alpn_protocols.is_empty(),
        "reloaded config also has no ALPN protocols"
    );
}

#[test]
#[should_panic(expected = "NotFound")]
fn load_tls_certs_missing_cert_file_panics() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("missing-cert.pem");
    let key = dir.path().join("missing-key.pem");
    drop(load_tls_certs(&cert, &key));
}

#[test]
#[should_panic(expected = "NotFound")]
fn load_tls_certs_missing_key_file_panics() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    drop(generate_tls_certs(&cert, &key));
    fs_err::remove_file(&key).unwrap();

    drop(load_tls_certs(&cert, &key));
}

#[test]
#[should_panic(expected = "InconsistentKeys")]
fn build_tls_server_config_with_mismatched_key_panics() {
    let dir = tempdir().unwrap();
    let (cert_a, _key_a) = generate_tls_certs(&dir.path().join("a-cert.pem"), &dir.path().join("a-key.pem"));
    let (_cert_b, key_b) = generate_tls_certs(&dir.path().join("b-cert.pem"), &dir.path().join("b-key.pem"));

    drop(build_tls_server_config(cert_a, key_b));
}
