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
    let from_generated = build_tls_server_config(gen_cert, gen_key);
    assert!(
        from_generated.alpn_protocols.is_empty(),
        "no ALPN protocols are configured by default"
    );

    // The freshly-written PEMs must load back into a der pair that also builds a valid config.
    let (loaded_cert, loaded_key) = load_tls_certs(&cert, &key);
    let from_loaded = build_tls_server_config(loaded_cert, loaded_key);
    assert!(
        from_loaded.alpn_protocols.is_empty(),
        "reloaded config also has no ALPN protocols"
    );
}
