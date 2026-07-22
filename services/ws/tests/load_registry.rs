//! Covers `load_registry`: the missing-file empty-registry fallback, a save/load round trip through the
//! session-less `BareRecord` shape (state, last-known IP, pending direct messages survive; sessions don't),
//! and the malformed-YAML error path.
#![cfg(test)]

use edge_toolkit::ws::{AgentConnectionState, AgentSummary, ConnectStatus};
use edge_toolkit::ws_server::RegistryError;
use et_ws_service::{WsAgentRegistry, load_registry};
use tempfile::tempdir;

#[test]
fn missing_file_yields_an_empty_registry() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("no-such-registry.yaml");

    let registry = load_registry(&path).unwrap();

    assert!(registry.list_agents().is_empty());
}

#[test]
fn round_trips_state_ip_and_pending_messages_but_not_sessions() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");

    let registry = WsAgentRegistry::default();
    let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
    let (agent_1_id, agent_1_status) = registry.connect_agent(None, "agent-1".to_string(), "10.0.0.5", tx1);
    let (agent_2_id, agent_2_status) = registry.connect_agent(None, "agent-2".to_string(), "10.0.0.6", tx2);
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
    );
    assert!(queued.is_some(), "agent-2 must exist to receive the queued message");
    registry.mark_disconnected("agent-2");
    registry.save(&path).unwrap();

    let loaded = load_registry(&path).unwrap();

    assert_eq!(
        loaded.list_agents(),
        vec![
            AgentSummary::new(
                "agent-1".to_string(),
                AgentConnectionState::Connected,
                Some("10.0.0.5".to_string())
            ),
            AgentSummary::new(
                "agent-2".to_string(),
                AgentConnectionState::Disconnected,
                Some("10.0.0.6".to_string())
            ),
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
