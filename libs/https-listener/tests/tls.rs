//! Covers the TLS bootstrap helpers: self-signed generation, PEM round-trip load, and rustls config build.
#![cfg(test)]

use et_https_listener::tls::{
    build_tls_server_config, generate_tls_certs, generate_tls_certs_covering, load_or_generate_tls_certs,
    load_tls_certs, uncovered_names,
};
use rustls::pki_types::CertificateDer;
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

/// Whether `needle` appears anywhere in `der`, as a subject alternative name's bytes do in a certificate.
fn holds(der: &[u8], needle: &[u8]) -> bool {
    der.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn a_certificate_covers_the_names_it_is_generated_for_beside_the_loopback_ones() {
    let dir = tempdir().unwrap();
    let names = ["10-1-2-3.nip.io".to_owned(), "10.1.2.3".to_owned()];
    let (cert, _key) = generate_tls_certs_covering(&dir.path().join("cert.pem"), &dir.path().join("key.pem"), &names);
    assert!(holds(cert.as_ref(), b"10-1-2-3.nip.io"));
    assert!(holds(cert.as_ref(), &[10, 1, 2, 3]));
    assert!(holds(cert.as_ref(), b"localhost"));
}

#[test]
fn a_pair_on_disk_is_loaded_and_a_missing_one_generated_once() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let (generated, _key) = load_or_generate_tls_certs(&cert, &key, &[]);
    assert!(cert.exists() && key.exists());
    let (loaded, _key) = load_or_generate_tls_certs(&cert, &key, &["other.example".to_owned()]);
    assert_eq!(generated, loaded);
    // A name the pair covers loads it just the same, with nothing to warn of.
    let (covered, _key) = load_or_generate_tls_certs(&cert, &key, &["localhost".to_owned()]);
    assert_eq!(generated, covered);
}

#[test]
fn a_certificate_without_its_key_is_replaced() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let (first, _key) = load_or_generate_tls_certs(&cert, &key, &[]);
    fs_err::remove_file(&key).unwrap();
    let (second, _key) = load_or_generate_tls_certs(&cert, &key, &[]);
    assert_ne!(first, second);
    assert!(key.exists());
}

#[test]
fn a_certificate_names_each_advertised_name_it_does_not_cover() {
    let dir = tempdir().unwrap();
    let names = ["10-1-2-3.nip.io".to_owned(), "10.1.2.3".to_owned()];
    let (cert, _key) = generate_tls_certs_covering(&dir.path().join("cert.pem"), &dir.path().join("key.pem"), &names);
    assert_eq!(uncovered_names(&cert, &names), Vec::<String>::new());
    let moved = ["10.1.2.3".to_owned(), "10.9.9.9".to_owned(), "not a name!".to_owned()];
    assert_eq!(uncovered_names(&cert, &moved), ["10.9.9.9", "not a name!"]);
    // A certificate that will not parse covers nothing.
    let garbled = CertificateDer::from(vec![1_u8, 2, 3]);
    assert_eq!(uncovered_names(&garbled, &names), names);
}

/// Check the key at `key` is a PEM private key that, on Unix, only its owner can read or write.
fn assert_private(key: &std::path::Path) {
    assert!(fs_err::read_to_string(key).unwrap().contains("PRIVATE KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs_err::metadata(key).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn a_generated_key_is_private_even_over_a_readable_one() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    drop(generate_tls_certs(&cert, &key));
    assert_private(&key);
    // A key left behind without its certificate, readable by all, is replaced rather than written through.
    fs_err::remove_file(&cert).unwrap();
    fs_err::write(&key, "old").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs_err::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    drop(load_or_generate_tls_certs(&cert, &key, &[]));
    assert_private(&key);
}

#[test]
#[should_panic(expected = "InconsistentKeys")]
fn build_tls_server_config_with_mismatched_key_panics() {
    let dir = tempdir().unwrap();
    let (cert_a, _key_a) = generate_tls_certs(&dir.path().join("a-cert.pem"), &dir.path().join("a-key.pem"));
    let (_cert_b, key_b) = generate_tls_certs(&dir.path().join("b-cert.pem"), &dir.path().join("b-key.pem"));

    drop(build_tls_server_config(cert_a, key_b));
}
