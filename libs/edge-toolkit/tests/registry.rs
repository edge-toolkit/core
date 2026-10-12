//! Covers the `AgentRegistry` persistence + session-lookup paths the ws-server integration tests skip:
//! save/load round-trip (including the missing-file and populated branches), reconnect, `agent_session`, and
//! the `with_pending_direct_messages` builder. `S = String` stands in for the runtime session handle (which is
//! `#[serde(skip)]`, so it is never persisted).
#![cfg(test)]

use std::collections::BTreeMap;

use edge_toolkit::ws::{AgentConnectionState, ConnectStatus};
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, MAX_AGENT_ID_LEN, is_valid_agent_id};
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
fn an_unsafe_id_is_replaced_and_an_unknown_disconnect_is_a_no_op() {
    let registry = AgentRegistry::<String>::default();

    // A requested id the registry has never seen, and that is not safe as a storage path segment, is not a reconnect:
    // the lookup misses, so the caller's `new_id` is inserted fresh and the status comes back Assigned.
    let (agent_id, status) = registry.connect_agent(
        Some("peer/escape".to_string()),
        "agent-fresh".to_string(),
        "127.0.0.1",
        "sess-a".to_string(),
    );
    assert_eq!(agent_id, "agent-fresh");
    assert_eq!(status, ConnectStatus::Assigned);

    // Disconnecting an id that was never registered is a no-op rather than an insert: the registry still holds exactly
    // the one agent above, still Connected.
    registry.mark_disconnected("never-registered");
    let summaries = registry.list_agents();
    assert_eq!(
        summaries.len(),
        1,
        "no record should have been created, got {summaries:?}"
    );
    assert_eq!(summaries[0].agent_id, "agent-fresh");
    assert_eq!(summaries[0].state, AgentConnectionState::Connected);

    // The known-id arm, asserted in this same `S = String` instantiation rather than left to the server tests that hit
    // it for real. The branch gate scores a generic function by the instantiation that covers the most of it (llvm-cov
    // merges an instantiation group's branch counts by taking the maximum, not the union), so the no-op arm above
    // covered here and the update arm covered only under the server's session type read as one arm each, never both:
    // `libs/edge-toolkit/src/ws_server.rs 9/10 branches` on commit
    // https://github.com/edge-toolkit/core/commit/c6c4fce73dd25aa58754963867ccf9523caae1bb at
    // https://github.com/edge-toolkit/core/actions/runs/35045764477/job/104635181514, while the lcov export, which sums
    // instantiations, showed every branch taken.
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
fn an_unknown_valid_id_is_adopted_as_the_agents_own() {
    let registry = AgentRegistry::<String>::default();

    // An id the hub has never seen but that is safe as a storage path segment is kept, so a headless agent keeps one
    // identity across hub restarts. It is still a first connection, so the status is Assigned.
    let (agent_id, status) = registry.connect_agent(
        Some("sensor-7".to_string()),
        "agent-generated".to_string(),
        "127.0.0.1",
        "sess-a".to_string(),
    );
    assert_eq!(agent_id, "sensor-7");
    assert_eq!(status, ConnectStatus::Assigned);
    assert_eq!(registry.agent_session("sensor-7").as_deref(), Some("sess-a"));
    assert_eq!(registry.agent_session("agent-generated"), None);
}

#[test]
fn a_displaced_session_cannot_disconnect_the_one_that_replaced_it() {
    let registry = AgentRegistry::<String>::default();
    let (agent_id, _assigned) = registry.connect_agent(
        Some("shared".to_string()),
        "unused".to_string(),
        "127.0.0.1",
        "sess-a".to_string(),
    );
    // A second connection claims the same id while the first still holds it, and takes it over.
    let (_same_id, status) = registry.connect_agent(
        Some(agent_id.clone()),
        "unused".to_string(),
        "127.0.0.2",
        "sess-b".to_string(),
    );
    assert_eq!(status, ConnectStatus::Reconnected);

    // The displaced connection closing leaves the id with its replacement.
    registry.mark_disconnected_if(&agent_id, &|session| session == "sess-a");
    assert_eq!(registry.agent_session(&agent_id).as_deref(), Some("sess-b"));
    assert_eq!(registry.list_agents()[0].state, AgentConnectionState::Connected);

    // The replacement closing does take it offline.
    registry.mark_disconnected_if(&agent_id, &|session| session == "sess-b");
    assert_eq!(registry.agent_session(&agent_id), None);
    assert_eq!(registry.list_agents()[0].state, AgentConnectionState::Disconnected);
}

#[test]
fn a_record_with_no_session_is_disconnected_whatever_the_test_says() {
    // Loaded from disk, a record carries no session, so no closing connection can prove it owns one.
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    let registry = AgentRegistry::<String>::default();
    let (agent_id, _assigned) = registry.connect_agent(None, "agent-1".to_string(), "127.0.0.1", "sess-a".to_string());
    registry.save(&path).unwrap();
    let reloaded = AgentRegistry::<String>::load(&path).unwrap();

    reloaded.mark_disconnected_if(&agent_id, &|_session| false);
    assert_eq!(reloaded.list_agents()[0].state, AgentConnectionState::Disconnected);
}

#[test]
fn agent_ids_are_held_to_a_single_safe_path_segment() {
    assert!(is_valid_agent_id("sensor-7"));
    assert!(is_valid_agent_id("a.b_c-d"));
    assert!(is_valid_agent_id(&"x".repeat(MAX_AGENT_ID_LEN)));
    assert!(!is_valid_agent_id(""));
    // Built from the char so the dot-only segments are not read as the relative path literals they would spell.
    let dot = '.'.to_string();
    assert!(!is_valid_agent_id(&dot));
    assert!(!is_valid_agent_id(&dot.repeat(2)));
    assert!(!is_valid_agent_id("a/b"));
    assert!(!is_valid_agent_id("a b"));
    assert!(!is_valid_agent_id(&"x".repeat(MAX_AGENT_ID_LEN + 1)));
}

#[test]
fn agent_record_with_pending_builder_replaces_the_map() {
    let record = AgentRecord::<String>::new(AgentConnectionState::Disconnected, None, None)
        .with_pending_direct_messages(BTreeMap::new());
    assert!(record.pending_direct_messages.is_empty());
}

#[test]
fn queued_direct_messages_from_one_sender_do_not_replace_each_other() {
    let registry = AgentRegistry::<String>::default();
    let (_recipient, _status) = registry.connect_agent(None, "recipient".to_string(), "10.0.0.1", "sess-r".to_string());
    let (_sender, _status) = registry.connect_agent(None, "sender".to_string(), "10.0.0.2", "sess-s".to_string());

    let (first, recipient_session) = registry
        .queue_direct(
            "msg-1".to_string(),
            "sender",
            "recipient",
            "t1".to_string(),
            serde_json::json!({"n": 1}),
        )
        .expect("first message queues");
    assert_eq!(first.message_id, "msg-1");
    assert!(recipient_session.is_some(), "recipient is connected");

    let (_second, _session) = registry
        .queue_direct(
            "msg-2".to_string(),
            "sender",
            "recipient",
            "t2".to_string(),
            serde_json::json!({"n": 2}),
        )
        .expect("second message queues");

    // Both messages wait: the second must not have replaced the first.
    let pending = registry.pending_messages_for("recipient");
    let ids = pending.iter().map(|p| p.message_id.as_str()).collect::<Vec<_>>();
    assert_eq!(ids, vec!["msg-1", "msg-2"], "both queued, in arrival order");

    // Acknowledging the first leaves the second queued for the recipient.
    let (acked, _session, sender) = registry
        .acknowledge_message("recipient", "msg-1")
        .expect("acknowledged");
    assert_eq!(acked, "msg-1");
    assert_eq!(sender, "sender");

    let pending = registry.pending_messages_for("recipient");
    assert_eq!(pending.len(), 1, "only the acknowledged message was removed");
    assert_eq!(pending[0].message_id, "msg-2");
    assert_eq!(pending[0].from_agent_id, "sender");
}

#[test]
fn entries_persisted_with_the_legacy_sender_key_still_acknowledge() {
    use edge_toolkit::ws_server::PendingDirectMessage;

    let registry = AgentRegistry::<String>::default();
    let (_recipient, _status) = registry.connect_agent(None, "recipient".to_string(), "10.0.0.1", "sess-r".to_string());
    let (_sender, _status) = registry.connect_agent(None, "sender".to_string(), "10.0.0.2", "sess-s".to_string());

    // A registry saved before queued messages were keyed by message id holds
    // sender-keyed entries; acknowledging one must still work.
    // Deserialised rather than built: the type is #[non_exhaustive] outside
    // the crate, and the persisted shape is exactly what load() hands back.
    let legacy_entry: PendingDirectMessage = serde_json::from_value(serde_json::json!({
        "message_id": "legacy-msg",
        "from_agent_id": "sender",
        "server_received_at": "t0",
        "message": {"n": 0},
    }))
    .expect("the persisted shape deserialises");
    let legacy = BTreeMap::from([("sender".to_string(), legacy_entry)]);
    let record = edge_toolkit::ws_server::AgentRecord::<String>::new(
        AgentConnectionState::Connected,
        Some("10.0.0.1".to_string()),
        None,
    )
    .with_pending_direct_messages(legacy);
    let registry = AgentRegistry::from_agents(BTreeMap::from([("recipient".to_string(), record)]));

    let (acked, _session, sender) = registry
        .acknowledge_message("recipient", "legacy-msg")
        .expect("legacy entry acknowledges");
    assert_eq!(acked, "legacy-msg");
    assert_eq!(sender, "sender");
    assert!(registry.pending_messages_for("recipient").is_empty());
}
