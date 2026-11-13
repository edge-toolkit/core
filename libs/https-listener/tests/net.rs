//! Covers the startup-banner IPv4 selection: filtering, class ranking, and the preferred-interface rules.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use et_https_listener::net::{NetError, candidate_ipv4s, rank_candidates, select_candidates};

fn ifa(name: &str, octets: [u8; 4]) -> (String, IpAddr) {
    (name.to_string(), IpAddr::V4(Ipv4Addr::from(octets)))
}

fn names(ranked: &[(String, IpAddr)]) -> Vec<&str> {
    ranked.iter().map(|(name, _)| name.as_str()).collect()
}

#[test]
fn filters_out_unusable_addresses() {
    let ranked = rank_candidates(
        vec![
            ifa("lo0", [127, 0, 0, 1]),
            ifa("en0", [169, 254, 13, 37]),
            ifa("en1", [0, 0, 0, 0]),
            ("utun0".to_string(), IpAddr::V6(Ipv6Addr::LOCALHOST)),
        ],
        None,
    );
    assert!(
        ranked.is_empty(),
        "loopback, link-local, unspecified, and IPv6 must all be dropped"
    );
}

#[test]
fn log_interface_ranks_first() {
    // Internet Sharing with a wired uplink: the phone scanning the QR code sits on the bridge100 network, so the
    // explicitly preferred hotspot gateway must beat the uplink NIC's LAN address.
    let ranked = rank_candidates(
        vec![ifa("en8", [10, 128, 116, 231]), ifa("bridge100", [192, 168, 2, 1])],
        Some("bridge100"),
    );
    assert_eq!(names(&ranked), ["bridge100", "en8"]);
    assert_eq!(ranked[0].1, IpAddr::V4(Ipv4Addr::new(192, 168, 2, 1)));
}

#[test]
fn nic_outranks_bridge_and_tunnels_without_a_preference() {
    // Normal LAN mode on a host running a VM: vmnet parks a bridge100 that must not hijack the QR code.
    let ranked = rank_candidates(
        vec![
            ifa("utun4", [10, 8, 0, 2]),
            ifa("bridge100", [192, 168, 64, 1]),
            ifa("en0", [10, 130, 108, 148]),
        ],
        None,
    );
    assert_eq!(names(&ranked), ["en0", "bridge100", "utun4"]);
}

#[test]
fn bridge_outranks_tunnels_without_a_preference() {
    // Hotspot mode with no uplink NIC address: the bridge is the best guess even with nothing configured.
    let ranked = rank_candidates(
        vec![ifa("utun4", [10, 8, 0, 2]), ifa("bridge100", [192, 168, 2, 1])],
        None,
    );
    assert_eq!(names(&ranked), ["bridge100", "utun4"]);
}

#[test]
fn enumeration_order_is_kept_within_a_class() {
    let ranked = rank_candidates(
        vec![ifa("en5", [192, 168, 1, 20]), ifa("en0", [10, 130, 108, 148])],
        None,
    );
    assert_eq!(
        names(&ranked),
        ["en5", "en0"],
        "sort must be stable: en5 enumerated first stays first"
    );
}

#[test]
fn missing_log_interface_is_a_startup_error() {
    let err = candidate_ipv4s(Some("no-such-interface0")).unwrap_err();
    assert!(matches!(err, NetError::InterfaceUnusable { .. }));
    let message = err.to_string();
    assert!(
        message.contains("no-such-interface0"),
        "error must name the missing interface: {message}"
    );
}

#[test]
fn no_preference_never_errors() {
    let _ranked = candidate_ipv4s(None).unwrap();
}

#[test]
fn a_failed_enumeration_is_an_error_only_when_an_interface_was_asked_for() {
    let failed = || Err(local_ip_address::Error::PlatformNotSupported("test".to_owned()));
    let err = select_candidates(failed(), Some("en0")).unwrap_err();
    assert!(matches!(err, NetError::Enumeration(_)));
    assert!(err.to_string().starts_with("failed to enumerate network interfaces"));
    assert_eq!(select_candidates(failed(), None).unwrap(), Vec::new());
}

#[test]
fn nothing_usable_is_no_candidate_and_no_error_without_a_preference() {
    let loopback_only = vec![ifa("lo", [127, 0, 0, 1])];
    assert_eq!(select_candidates(Ok(loopback_only), None).unwrap(), Vec::new());
}

#[test]
fn an_asked_for_interface_among_nothing_usable_says_so() {
    let err = select_candidates(Ok(Vec::new()), Some("en0")).unwrap_err();
    assert!(err.to_string().ends_with("usable candidates: (none)"));
}

#[test]
fn an_asked_for_interface_that_is_usable_is_ranked_first() {
    let enumerated = vec![ifa("en0", [192, 168, 1, 2]), ifa("wg0", [10, 0, 0, 2])];
    let ranked = select_candidates(Ok(enumerated), Some("wg0")).unwrap();
    assert_eq!(names(&ranked), ["wg0", "en0"]);
}
