//! Covers `load_registry`: the missing-file empty-registry fallback, a save/load round trip through the
//! session-less `BareRecord` shape (state, last-known IP, pending direct messages survive; sessions don't),
//! and the malformed-YAML error path.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use edge_toolkit::ws::{AgentConnectionState, AgentSummary, ConnectStatus};
use edge_toolkit::ws_server::{MessageRetention, RegistryError};
use et_ws_service::{WsAgentRegistry, load_registry};
use tempfile::tempdir;

const AGENT_1_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5));
const AGENT_2_IP: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 6));

#[test]
fn missing_file_yields_an_empty_registry() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("no-such-registry.yaml");

    let registry = load_registry(&path).unwrap();

    assert_eq!(registry.list_agents(), Vec::<AgentSummary>::new());
}

#[test]
fn round_trips_state_ip_and_pending_messages_but_not_sessions() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");

    let registry = WsAgentRegistry::default();
    let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
    let (agent_1_id, agent_1_status) = registry.connect_agent(None, "agent-1".to_string(), AGENT_1_IP, tx1);
    let (agent_2_id, agent_2_status) = registry.connect_agent(None, "agent-2".to_string(), AGENT_2_IP, tx2);
    assert_eq!(
        (agent_1_id.as_str(), agent_1_status),
        ("agent-1", ConnectStatus::Assigned)
    );
    assert_eq!(
        (agent_2_id.as_str(), agent_2_status),
        ("agent-2", ConnectStatus::Assigned)
    );
    let queued = registry.queue_direct(
        "msg-1".to_string(),
        "agent-1",
        "agent-2",
        "2026-07-22T00:00:00Z".to_string(),
        serde_json::json!({"hello": "world"}),
        MessageRetention::default(),
    );
    assert!(queued.is_some(), "agent-2 must exist to receive the queued message");
    registry.mark_disconnected("agent-2");
    registry.save(&path).unwrap();

    let loaded = load_registry(&path).unwrap();

    assert_eq!(
        loaded.list_agents(),
        vec![
            AgentSummary::new("agent-1".to_string(), AgentConnectionState::Connected, AGENT_1_IP),
            AgentSummary::new("agent-2".to_string(), AgentConnectionState::Disconnected, AGENT_2_IP),
        ]
    );
    assert!(
        loaded.agent_session("agent-1").is_none(),
        "sessions are never persisted, so a reloaded agent must have none even though it was Connected"
    );

    let pending = loaded.pending_messages_for("agent-2");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, "msg-1");
    assert_eq!(pending[0].from_agent_id, "agent-1");
    assert_eq!(pending[0].message, serde_json::json!({"hello": "world"}));
}

#[test]
fn the_blocklist_round_trips_and_an_older_bare_registry_still_loads() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    let registry = WsAgentRegistry::default();
    assert!(registry.blocklist.block(AGENT_2_IP));
    registry.save(&path).unwrap();
    assert!(load_registry(&path).unwrap().blocklist.is_blocked(AGENT_2_IP));

    fs_err::write(&path, "agent-1:\n  state: connected\n  last_known_ip: 10.0.0.5\n").unwrap();
    let bare = load_registry(&path).unwrap();
    assert_eq!(bare.agent_ip("agent-1").unwrap(), AGENT_1_IP);
    assert!(bare.blocklist.ips().is_empty());
}

#[test]
fn malformed_yaml_is_a_registry_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    fs_err::write(&path, "- this\n- is\n- a list, not a map of agents\n").unwrap();

    match load_registry(&path) {
        Err(RegistryError::Yaml(_)) => {}
        Err(other) => panic!("expected RegistryError::Yaml, got {other:?}"),
        Ok(_) => panic!("expected an error for malformed YAML"),
    }
}
