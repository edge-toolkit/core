#![cfg_attr(doc, doc = include_str!("../README.md"))]

use std::net::IpAddr;
use std::path::PathBuf;

pub mod app;
pub mod measure;
pub mod registry;

/// Collect GPU probe signatures into a registry over HTTPS.
#[derive(Debug, clap::Parser)]
#[command(name = "et-gpu-probe-server", version)]
#[non_exhaustive]
pub struct Args {
    /// The registry file submissions are added to, such as `libs/gpu-probe/signatures.yaml`.
    #[arg(long)]
    pub registry: PathBuf,
    /// A directory of static files, such as a probe page, to serve at `/`.
    ///
    /// Absent, the server serves only the form, the endpoint and the registry: there is no page to serve.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// The address to listen on: `0.0.0.0` for other devices on the LAN to reach it.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
    /// The port to listen on.
    #[arg(long, default_value_t = 8443)]
    pub port: u16,
    /// The PEM certificate to serve, generated self-signed with the key when either is missing.
    #[arg(long)]
    pub cert: PathBuf,
    /// The PEM private key of the certificate.
    #[arg(long)]
    pub key: PathBuf,
    /// Another name to advertise the server at, which a generated certificate covers too.
    #[arg(long)]
    pub public_host: Vec<String>,
    /// Serve plain HTTP instead of HTTPS, which only a loopback `--bind` may.
    ///
    /// A browser treats a loopback origin as secure, so WebGPU still runs there.
    ///
    /// `--cert` and `--key` are not read.
    #[arg(long)]
    pub http: bool,
}

/// Why the server would not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StartError {
    /// Plain HTTP was asked for on an address other devices could reach, where a browser would not run WebGPU.
    #[error("--http serves only a loopback --bind, not {0}: a browser treats only a loopback origin as secure")]
    NotLoopback(IpAddr),
}

impl Args {
    /// Check the arguments can be served as given.
    pub const fn check(&self) -> Result<(), StartError> {
        if self.http && !self.bind.is_loopback() {
            return Err(StartError::NotLoopback(self.bind));
        }
        Ok(())
    }
}

/// The `nip.io` name that resolves to `address`, such as `10-1-2-3.nip.io` for `10.1.2.3`.
///
/// It is a real domain for a LAN address, for trying a browser defence that treats a bare IP differently.
#[must_use]
pub fn nip_io(address: IpAddr) -> String {
    format!("{}.nip.io", address.to_string().replace(['.', ':'], "-"))
}

/// Every name a generated certificate covers beyond the loopback ones, and the server is advertised at.
///
/// That is each of `public_hosts`, then each of the `lan` addresses and its [`nip_io`] name.
#[must_use]
pub fn advertised_names(public_hosts: &[String], lan: &[IpAddr]) -> Vec<String> {
    public_hosts
        .iter()
        .cloned()
        .chain(lan.iter().flat_map(|address| [address.to_string(), nip_io(*address)]))
        .collect()
}
