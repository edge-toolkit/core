use std::path::Path;

use rustls::pki_types::pem::PemObject;

type CertKeyPair = (
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
);

pub fn load_tls_certs(cert_filename: &Path, key_filename: &Path) -> CertKeyPair {
    let cert_der = rustls::pki_types::CertificateDer::from_pem_file(cert_filename).unwrap();
    let key_der = rustls::pki_types::PrivateKeyDer::from_pem_file(key_filename).unwrap();
    (cert_der, key_der)
}

pub fn generate_tls_certs(cert_filename: &Path, key_filename: &Path) -> CertKeyPair {
    let certified = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ])
    .unwrap();
    std::fs::write(cert_filename, certified.cert.pem()).unwrap();
    std::fs::write(key_filename, certified.signing_key.serialize_pem()).unwrap();
    let cert_der = certified.cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::from(rustls::pki_types::PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    (cert_der, key_der)
}

pub fn build_tls_server_config(
    cert_der: rustls::pki_types::CertificateDer<'static>,
    key_der: rustls::pki_types::PrivateKeyDer<'static>,
) -> rustls::ServerConfig {
    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .unwrap()
}
