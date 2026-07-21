//! Discovers which of the host's IPv4 addresses the startup banner URLs and QR code should advertise.

use std::net::IpAddr;

use tracing::{info, warn};

/// Errors from startup network-interface selection.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NetError {
    /// The OS interface enumeration failed while an explicit interface preference was configured.
    #[error("failed to enumerate network interfaces while NET_LOG_INTERFACE is set: {0}")]
    Enumeration(#[from] local_ip_address::Error),
    /// The explicitly configured interface is absent or holds no usable IPv4 address.
    #[error("configured interface {name} is not up with a usable IPv4 address; usable candidates: {usable}")]
    InterfaceUnusable { name: String, usable: String },
}

/// Filters an interface list to reachable IPv4 addresses and orders them by likely client reachability.
///
/// Keeps only non-loopback, non-link-local, non-unspecified IPv4 addresses, then sorts them by interface
/// class, preserving enumeration order within each class: the `log_interface` name (if given) first,
/// then `en*` (Wi-Fi / Ethernet), then `bridge*`, then everything else (VPN tunnels and the like). The
/// `bridge*` class still outranks the tunnels because when macOS Internet Sharing is active and the Wi-Fi
/// NIC holds no address, its `bridge100` gateway is the best guess even without an explicit preference.
#[must_use]
pub fn rank_candidates(ifas: Vec<(String, IpAddr)>, log_interface: Option<&str>) -> Vec<(String, IpAddr)> {
    let mut candidates: Vec<(String, IpAddr)> = ifas
        .into_iter()
        .filter(|(_, addr)| match addr {
            IpAddr::V4(ipv4) => !ipv4.is_loopback() && !ipv4.is_link_local() && !ipv4.is_unspecified(),
            IpAddr::V6(_) => false,
        })
        .collect();
    candidates.sort_by_key(|(name, _)| {
        if log_interface == Some(name.as_str()) {
            0_u8
        } else if name.starts_with("en") {
            1
        } else if name.starts_with("bridge") {
            2
        } else {
            3
        }
    });
    candidates
}

fn format_ifas(ifas: &[(String, IpAddr)]) -> String {
    if ifas.is_empty() {
        return "(none)".to_string();
    }
    let entries: Vec<String> = ifas.iter().map(|(name, addr)| format!("{name}={addr}")).collect();
    entries.join(", ")
}

/// Enumerates the host's network interfaces and returns the ranked IPv4 candidates for the startup banner.
///
/// Logs the complete raw enumeration (every interface and address, before any filtering) so a machine
/// with no usable candidate can be diagnosed from the startup log alone -- the offline-hotspot demo is
/// exactly the situation where no second machine is available to poke around from. An explicitly
/// configured `log_interface` is a hard requirement: if it is absent or holds no usable IPv4 address,
/// this returns an error (aborting startup) rather than silently advertising some other address.
pub fn candidate_ipv4s(log_interface: Option<&str>) -> Result<Vec<(String, IpAddr)>, NetError> {
    let ifas = match local_ip_address::list_afinet_netifas() {
        Ok(ifas) => ifas,
        Err(e) => {
            if log_interface.is_some() {
                return Err(NetError::Enumeration(e));
            }
            warn!("Failed to enumerate network interfaces: {e}");
            return Ok(Vec::new());
        }
    };
    info!("Enumerated network interfaces (pre-filter): {}", format_ifas(&ifas));
    let ranked = rank_candidates(ifas, log_interface);
    match log_interface {
        Some(preferred) => {
            if !ranked.iter().any(|(name, _)| name == preferred) {
                return Err(NetError::InterfaceUnusable {
                    name: preferred.to_string(),
                    usable: format_ifas(&ranked),
                });
            }
        }
        None => {
            if ranked.is_empty() {
                warn!("Every enumerated address was filtered out (IPv6, loopback, link-local, or unspecified)");
            }
        }
    }
    Ok(ranked)
}
