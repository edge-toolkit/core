//! Covers the `AgentRegistry` persistence the `std` feature adds: the save/load round-trip (including the
//! missing-file and populated branches), the blocklist saved beside the agents, and both file layouts. `S = String`
//! stands in for the runtime session handle (which is `#[serde(skip)]`, so it is never persisted).
#![cfg(test)]

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};

use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, RegistryError, read_registry_file};
use tempfile::tempdir;

const FIRST_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const SECOND_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

#[test]
fn save_load_roundtrip_and_session_lookup() {
    let registry = AgentRegistry::<String>::default();

    // Fresh connection (Assigned), then a reconnect for the same id which swaps in a new session.
    let (agent_id, _assigned) = registry.connect_agent(None, "agent-1".to_string(), FIRST_IP, "sess-a".to_string());
    let (_same_id, _reconnected) = registry.connect_agent(
        Some(agent_id.clone()),
        "agent-x".to_string(),
        SECOND_IP,
        "sess-b".to_string(),
    );

    assert_eq!(
        registry.agent_session(&agent_id).as_deref(),
        Some("sess-b"),
        "reconnect keeps the newest session"
    );
    assert_eq!(registry.agent_session("nobody"), None, "unknown agent has no session");

    // Persist and reload. Sessions are #[serde(skip)], so they return as None, but the agent survives.
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    registry.save(&path).unwrap();
    let reloaded = AgentRegistry::<String>::load(&path).unwrap();
    assert_eq!(reloaded.agent_session(&agent_id), None, "sessions are not persisted");
    assert_eq!(reloaded.list_agents()[0].last_known_ip, SECOND_IP);

    // load() on a missing file yields an empty registry rather than erroring.
    let empty = AgentRegistry::<String>::load(&dir.path().join("absent.yaml")).unwrap();
    assert_eq!(empty.agent_session(&agent_id), None);
}

#[test]
fn the_blocklist_is_saved_and_loaded_with_the_agents() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    let registry = AgentRegistry::<String>::default();
    let (agent_id, _assigned) = registry.connect_agent(None, "agent-1".to_string(), FIRST_IP, "sess-a".to_string());
    assert!(registry.blocklist.block(SECOND_IP));
    registry.save(&path).unwrap();

    let reloaded = AgentRegistry::<String>::load(&path).unwrap();
    assert_eq!(reloaded.blocklist.ips(), BTreeSet::from([SECOND_IP]));
    assert_eq!(reloaded.agent_ip(&agent_id).unwrap(), FIRST_IP);
}

#[test]
fn a_registry_saved_before_the_blocklist_still_loads() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    fs_err::write(&path, "agent-1:\n  state: disconnected\n  last_known_ip: 127.0.0.1\n").unwrap();

    let file = read_registry_file::<AgentRecord<String>>(&path).unwrap();
    assert_eq!(file.agents["agent-1"].last_known_ip, FIRST_IP);
    assert!(file.blocked_ips.is_empty());
    assert_eq!(AgentRegistry::<String>::load(&path).unwrap().list_agents().len(), 1);
}

#[test]
fn a_registry_that_is_neither_layout_is_an_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    fs_err::write(&path, "- a list, not a registry\n").unwrap();

    assert!(matches!(
        AgentRegistry::<String>::load(&path),
        Err(RegistryError::Yaml(_))
    ));
}
