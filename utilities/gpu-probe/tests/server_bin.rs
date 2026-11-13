//! Covers starting the `et-gpu-probe-server` binary on the paths that end by themselves: a refused `--http` address,
//! and a port another listener already holds, over plain HTTP and over HTTPS with a certificate generated first.
#![cfg(test)]

use std::net::TcpListener;
use std::process::Output;

use tempfile::TempDir;

/// The server's output when started with `args` and a registry, certificate and key in `dir`.
///
/// Every start here is meant to fail, so the raw exit status is what is asserted on.
fn started(dir: &TempDir, args: &[&str]) -> Output {
    let path = |name: &str| dir.path().join(name);
    std::process::Command::new(env!("CARGO_BIN_EXE_et-gpu-probe-server"))
        .arg("--registry")
        .arg(path("signatures.yaml"))
        .arg("--cert")
        .arg(path("cert.pem"))
        .arg("--key")
        .arg(path("key.pem"))
        .args(args)
        // The log line a test looks for is at info, which a filter inherited from the caller could hide.
        .env("RUST_LOG", "info")
        .output()
        .unwrap()
}

/// A port on the loopback address a listener holds for as long as the returned listener lives.
fn taken() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    (listener, port)
}

#[test]
fn plain_http_off_the_loopback_address_is_refused() {
    let dir = TempDir::new().unwrap();
    let output = started(&dir, &["--http", "--bind", "0.0.0.0"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("0.0.0.0"), "{stderr}");
}

#[test]
fn plain_http_on_a_taken_port_fails_to_bind() {
    let dir = TempDir::new().unwrap();
    let (_held, port) = taken();
    let output = started(&dir, &["--http", "--port", &port]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("adding submissions to"), "{stdout}");
}

#[test]
fn https_on_a_taken_port_generates_the_certificate_then_fails_to_bind() {
    let dir = TempDir::new().unwrap();
    let (_held, port) = taken();
    let output = started(&dir, &["--port", &port]);
    assert!(!output.status.success());
    assert!(dir.path().join("cert.pem").exists());
    assert!(dir.path().join("key.pem").exists());
}
