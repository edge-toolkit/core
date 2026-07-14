//! Covers the `AgentRegistry` persistence + session-lookup paths the ws-server integration tests skip:
//! save/load round-trip (including the missing-file and populated branches), reconnect, `agent_session`,
//! and the `with_pending_direct_messages` builder. `S = String` stands in for the runtime session handle
//! (which is `#[serde(skip)]`, so it is never persisted).
#![cfg(test)]

use std::collections::BTreeMap;

use edge_toolkit::ws::AgentConnectionState;
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry};
use tempfile::tempdir;

#[test]
fn save_load_roundtrip_and_session_lookup() {
    let registry = AgentRegistry::<String>::default();

    // Fresh connection (Assigned), then a reconnect for the same id which swaps in a new session.
    let (agent_id, _assigned) = registry.connect_agent(None, "agent-1".to_string(), "127.0.0.1", "sess-a".to_string());
    let (_same_id, _reconnected) = registry.connect_agent(
        Some(agent_id.clone()),
        "agent-x".to_string(),
        "127.0.0.2",
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

    // load() on a missing file yields an empty registry rather than erroring.
    let empty = AgentRegistry::<String>::load(&dir.path().join("absent.yaml")).unwrap();
    assert_eq!(empty.agent_session(&agent_id), None);
}

#[test]
fn agent_record_with_pending_builder_replaces_the_map() {
    let record = AgentRecord::<String>::new(AgentConnectionState::Disconnected, None, None)
        .with_pending_direct_messages(BTreeMap::new());
    assert!(record.pending_direct_messages.is_empty());
}
