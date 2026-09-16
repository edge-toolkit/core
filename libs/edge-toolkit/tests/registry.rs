//! Covers the `AgentRegistry` persistence + session-lookup paths the ws-server integration tests skip:
//! save/load round-trip (including the missing-file and populated branches), reconnect, `agent_session`,
//! and the `with_pending_direct_messages` builder. `S = String` stands in for the runtime session handle
//! (which is `#[serde(skip)]`, so it is never persisted).
#![cfg(test)]

use std::collections::BTreeMap;

use edge_toolkit::ws::{AgentConnectionState, ConnectStatus};
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
fn unknown_ids_fall_through_to_the_assign_and_no_op_paths() {
    let registry = AgentRegistry::<String>::default();

    // A requested id the registry has never seen is not a reconnect: the lookup misses, so the caller's
    // `new_id` is inserted fresh and the status comes back Assigned. Getting this wrong would hand a
    // reconnecting client someone else's identity, or resurrect an id the server has forgotten.
    let (agent_id, status) = registry.connect_agent(
        Some("never-registered".to_string()),
        "agent-fresh".to_string(),
        "127.0.0.1",
        "sess-a".to_string(),
    );
    assert_eq!(agent_id, "agent-fresh");
    assert_eq!(status, ConnectStatus::Assigned);

    // Disconnecting an id that was never registered is a no-op rather than an insert: the registry still
    // holds exactly the one agent above, still Connected.
    registry.mark_disconnected("never-registered");
    let summaries = registry.list_agents();
    assert_eq!(
        summaries.len(),
        1,
        "no record should have been created, got {summaries:?}"
    );
    assert_eq!(summaries[0].agent_id, "agent-fresh");
    assert_eq!(summaries[0].state, AgentConnectionState::Connected);

    // The known-id arm, asserted in this same `S = String` instantiation rather than left to the server tests
    // that hit it for real. The branch gate scores a generic function by the instantiation that covers the most
    // of it (llvm-cov merges an instantiation group's branch counts by taking the maximum, not the union), so
    // the no-op arm above covered here and the update arm covered only under the server's session type read as
    // one arm each, never both: `libs/edge-toolkit/src/ws_server.rs 9/10 branches` on commit
    // c6c4fce73dd25aa58754963867ccf9523caae1bb at
    // https://github.com/edge-toolkit/core/actions/runs/35045764477/job/104635181514, while the lcov export,
    // which sums instantiations, showed every branch taken.
    registry.mark_disconnected("agent-fresh");
    let summaries = registry.list_agents();
    assert_eq!(summaries[0].state, AgentConnectionState::Disconnected);
    assert_eq!(
        registry.agent_session("agent-fresh"),
        None,
        "disconnecting must drop the session handle"
    );
}

#[test]
fn agent_record_with_pending_builder_replaces_the_map() {
    let record = AgentRecord::<String>::new(AgentConnectionState::Disconnected, None, None)
        .with_pending_direct_messages(BTreeMap::new());
    assert!(record.pending_direct_messages.is_empty());
}
