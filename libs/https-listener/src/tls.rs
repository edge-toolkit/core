//! The certificate an HTTPS listener serves: loaded from PEM files, or generated self-signed and written to them.
#![expect(
    clippy::unwrap_used,
    reason = "TLS bootstrap helpers panic on invalid PEM / cert-gen failure intentionally; callers are main + tests"
)]

use std::path::Path;

use rustls::pki_types::pem::PemObject as _;
#[cfg(feature = "tls-self-signed")]
use tracing::{info, warn};

type CertKeyPair = (
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
);

#[must_use]
pub fn load_tls_certs(cert_filename: &Path, key_filename: &Path) -> CertKeyPair {
    let cert_der = rustls::pki_types::CertificateDer::from_pem_file(cert_filename).unwrap();
    let key_der = rustls::pki_types::PrivateKeyDer::from_pem_file(key_filename).unwrap();
    (cert_der, key_der)
}

/// Generate a self-signed pair for `localhost`, `127.0.0.1` and `::1`, writing it to the two files.
#[cfg(feature = "tls-self-signed")]
#[must_use]
pub fn generate_tls_certs(cert_filename: &Path, key_filename: &Path) -> CertKeyPair {
    generate_tls_certs_covering(cert_filename, key_filename, &[])
}

/// Generate a self-signed pair for the loopback names and `names`, writing it to the two files.
///
/// Each of `names` is a DNS name or an IP address, such as a LAN address a phone reaches the server at, which a browser
/// then finds in the certificate rather than refusing it as issued for another host.
#[cfg(feature = "tls-self-signed")]
#[must_use]
pub fn generate_tls_certs_covering(cert_filename: &Path, key_filename: &Path, names: &[String]) -> CertKeyPair {
    let loopback = ["localhost", "127.0.0.1", "::1"].map(str::to_owned);
    let certified = rcgen::generate_simple_self_signed([loopback.as_slice(), names].concat()).unwrap();
    fs_err::write(cert_filename, certified.cert.pem()).unwrap();
    write_private(key_filename, certified.signing_key.serialize_pem().as_bytes());
    let cert_der = certified.cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::from(rustls::pki_types::PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    (cert_der, key_der)
}

/// Write `contents` to a new file at `path` that only its owner can read, replacing any file there.
///
/// On Unix the file is created with mode `0600`, so the key is never readable by anyone else, not even for the moment
/// between its creation and a later change of mode. Windows gives a new file the access list of the directory it is
/// made in, which in a user's own tree already admits only that user and the administrators.
#[cfg(feature = "tls-self-signed")]
#[expect(
    clippy::single_call_fn,
    reason = "writing a secret file owner-only is a step of its own, apart from generating what goes in it"
)]
fn write_private(path: &Path, contents: &[u8]) {
    use std::io::Write as _;

    // A file already there keeps its own mode when written over, so it is removed rather than truncated; it may well
    // not exist, and the open below reports any real problem.
    let _absent: Result<(), std::io::Error> = fs_err::remove_file(path);
    let mut new_file = fs_err::OpenOptions::new();
    let options = new_file.write(true).create_new(true);
    #[cfg(unix)]
    let options = {
        use fs_err::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600)
    };
    options.open(path).unwrap().write_all(contents).unwrap();
}

/// Each of `names` the certificate `cert` is not valid for, in the order given.
///
/// A name that is neither a DNS name nor an IP address cannot be covered by any certificate, and is counted with them.
#[cfg(feature = "tls-self-signed")]
#[must_use]
pub fn uncovered_names(cert: &rustls::pki_types::CertificateDer<'_>, names: &[String]) -> Vec<String> {
    let parsed = webpki::EndEntityCert::try_from(cert);
    names
        .iter()
        .filter(|name| {
            let server = rustls::pki_types::ServerName::try_from(name.as_str());
            !matches!((&parsed, server), (Ok(end), Ok(server)) if end.verify_is_valid_for_subject_name(&server).is_ok())
        })
        .cloned()
        .collect()
}

/// Warn of each of `names` the loaded certificate `cert` does not cover, and how to have a pair made that does.
#[cfg(feature = "tls-self-signed")]
#[expect(
    clippy::single_call_fn,
    reason = "the stale-certificate warning is a step of loading a pair, kept apart from the choice to load one"
)]
fn warn_if_stale(cert: &rustls::pki_types::CertificateDer<'_>, names: &[String], cert_filename: &Path, key: &Path) {
    let missing = uncovered_names(cert, names);
    if !missing.is_empty() {
        warn!(
            "TLS certificate {} does not cover {}; delete it and {} to have one generated that does",
            cert_filename.display(),
            missing.join(", "),
            key.display()
        );
    }
}

/// The pair in the two files when both exist, or else a newly generated one.
///
/// A generated pair is self-signed for the loopback names and `names`, and written to the two files. A loaded pair is
/// served as it is, since it may be one an operator gave; each of `names` it does not cover -- a LAN address that has
/// changed since it was generated -- is warned of, with how to have a new one made.
#[cfg(feature = "tls-self-signed")]
#[must_use]
pub fn load_or_generate_tls_certs(cert_filename: &Path, key_filename: &Path, names: &[String]) -> CertKeyPair {
    let (done, pair) = if cert_filename.exists() && key_filename.exists() {
        let pair = load_tls_certs(cert_filename, key_filename);
        warn_if_stale(&pair.0, names, cert_filename, key_filename);
        ("Loaded", pair)
    } else {
        (
            "Generated a self-signed",
            generate_tls_certs_covering(cert_filename, key_filename, names),
        )
    };
    let report = format!(
        "{done} TLS certificate {} and key {}",
        cert_filename.display(),
        key_filename.display()
    );
    info!("{report}");
    pair
}

#[must_use]
pub fn build_tls_server_config(
    cert_der: rustls::pki_types::CertificateDer<'static>,
    key_der: rustls::pki_types::PrivateKeyDer<'static>,
) -> rustls::ServerConfig {
    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .unwrap()
}
