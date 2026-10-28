//! `et_cli::block_ip` and `et_cli::evict_agent` against an in-process hub: the hub's answer on success, and a
//! refused token as an error.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr};

use et_cli::{CliError, block_ip, evict_agent};
use et_ws_test_server::{ADMIN_TOKEN, connect_agent};
use reqwest::StatusCode;
use tokio::runtime::Runtime;

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn is_unauthorized(error: &CliError) -> bool {
    matches!(error, CliError::HubRequest(error) if error.status() == Some(StatusCode::UNAUTHORIZED))
}

#[test]
fn an_evicted_agents_address_is_reported_and_a_wrong_token_is_refused() {
    let server = et_ws_test_server::start();
    let (_socket, agent_id) = runtime().block_on(connect_agent(&server.ws_url));

    assert!(is_unauthorized(
        &evict_agent(&server.base_url, &agent_id, "wrong-token").unwrap_err()
    ));

    let evicted = evict_agent(&format!("{}/", server.base_url), &agent_id, ADMIN_TOKEN).unwrap();
    assert_eq!(evicted.agent_id, agent_id);
    assert_eq!(evicted.blocked_ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_eq!(
        evicted.to_string(),
        format!("Evicted agent {agent_id}; blocked IP 127.0.0.1")
    );
}

#[test]
fn a_blocked_address_is_refused_storage_and_a_wrong_token_is_refused() {
    let server = et_ws_test_server::start();
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let stored = format!("{}/storage/agent-1/file.txt", server.base_url);
    let status = || runtime().block_on(async { reqwest::get(&stored).await.unwrap().status() });

    assert!(is_unauthorized(
        &block_ip(&server.base_url, loopback, "wrong-token").unwrap_err()
    ));
    assert_ne!(status(), StatusCode::FORBIDDEN);

    block_ip(&format!("{}/", server.base_url), loopback, ADMIN_TOKEN).unwrap();
    assert_eq!(status(), StatusCode::FORBIDDEN);
}
