//! Covers the `AgentRegistry` persistence + session-lookup paths the ws-server integration tests skip:
//! save/load round-trip (including the missing-file and populated branches), reconnect, `agent_session`, and
//! the `with_pending_direct_messages` builder. `S = String` stands in for the runtime session handle (which is
//! `#[serde(skip)]`, so it is never persisted).
#![cfg(test)]

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use edge_toolkit::ws::{AgentConnectionState, ConnectStatus};
use edge_toolkit::ws_server::{
    AcknowledgeError, AgentRecord, AgentRegistry, MAX_AGENT_ID_LEN, MessageRetention, is_valid_agent_id,
};
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
fn an_unsafe_id_is_replaced_and_an_unknown_disconnect_is_a_no_op() {
    let registry = AgentRegistry::<String>::default();

    // A requested id the registry has never seen, and that is not safe as a storage path segment, is not a reconnect:
    // the lookup misses, so the caller's `new_id` is inserted fresh and the status comes back Assigned.
    let (agent_id, status) = registry.connect_agent(
        Some("peer/escape".to_string()),
        "agent-fresh".to_string(),
        FIRST_IP,
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
        FIRST_IP,
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
        FIRST_IP,
        "sess-a".to_string(),
    );
    // A second connection claims the same id while the first still holds it, and takes it over.
    let (_same_id, status) = registry.connect_agent(
        Some(agent_id.clone()),
        "unused".to_string(),
        SECOND_IP,
        "sess-b".to_string(),
    );
    assert_eq!(status, ConnectStatus::Reconnected);

    // The displaced connection closing leaves the id with its replacement.
    assert!(!registry.mark_disconnected_if(&agent_id, &|session| session == "sess-a"));
    assert_eq!(registry.agent_session(&agent_id).as_deref(), Some("sess-b"));
    assert_eq!(registry.list_agents()[0].state, AgentConnectionState::Connected);

    // The replacement closing does take it offline.
    assert!(registry.mark_disconnected_if(&agent_id, &|session| session == "sess-b"));
    assert_eq!(registry.agent_session(&agent_id), None);
    assert_eq!(registry.list_agents()[0].state, AgentConnectionState::Disconnected);
}

#[test]
fn a_record_with_no_session_is_disconnected_whatever_the_test_says() {
    // Loaded from disk, a record carries no session, so no closing connection can prove it owns one.
    let dir = tempdir().unwrap();
    let path = dir.path().join("registry.yaml");
    let registry = AgentRegistry::<String>::default();
    let (agent_id, _assigned) = registry.connect_agent(None, "agent-1".to_string(), FIRST_IP, "sess-a".to_string());
    registry.save(&path).unwrap();
    let reloaded = AgentRegistry::<String>::load(&path).unwrap();

    assert!(reloaded.mark_disconnected_if(&agent_id, &|_session| false));
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
    let record = AgentRecord::<String>::new(AgentConnectionState::Disconnected, FIRST_IP, None)
        .with_pending_direct_messages(BTreeMap::new());
    assert!(record.pending_direct_messages.is_empty());
}

/// A registry holding a connected `recipient` and a connected `sender`, each with a session named after it.
fn sender_and_recipient() -> AgentRegistry<String> {
    let registry = AgentRegistry::<String>::default();
    let (_recipient, _assigned) = registry.connect_agent(
        Some("recipient".to_string()),
        String::default(),
        FIRST_IP,
        "r".to_string(),
    );
    let (_sender, _assigned) = registry.connect_agent(
        Some("sender".to_string()),
        String::default(),
        SECOND_IP,
        "s".to_string(),
    );
    registry
}

/// Queue two messages from `sender` to `recipient` under `retention`, returning the ids left pending.
fn queue_two(registry: &AgentRegistry<String>, retention: MessageRetention) -> Vec<String> {
    for message_id in ["m1", "m2"] {
        let (pending, session) = registry
            .queue_direct(
                message_id.to_string(),
                "sender",
                "recipient",
                "2026-10-26T00:00:00Z".to_string(),
                serde_json::json!({"id": message_id}),
                retention,
            )
            .unwrap();
        assert_eq!(pending.message_id, message_id);
        assert_eq!(session.as_deref(), Some("r"));
    }
    let pending = registry.pending_messages_for("recipient");
    pending.into_iter().map(|message| message.message_id).collect()
}

#[test]
fn latest_per_sender_retention_replaces_the_earlier_message() {
    let registry = sender_and_recipient();
    assert_eq!(MessageRetention::default(), MessageRetention::LatestPerSender);

    assert_eq!(queue_two(&registry, MessageRetention::LatestPerSender), ["m2"]);
    assert!(matches!(
        registry.acknowledge_message("recipient", "m1"),
        Err(AcknowledgeError::NoPendingMessage)
    ));
}

#[test]
fn all_retention_keeps_every_message_until_acknowledged() {
    let registry = sender_and_recipient();

    assert_eq!(queue_two(&registry, MessageRetention::All), ["m1", "m2"]);
    let (acknowledged, sender_session, sender) = registry.acknowledge_message("recipient", "m1").unwrap();
    assert_eq!(
        (acknowledged.as_str(), sender_session.as_deref(), sender.as_str()),
        ("m1", Some("s"), "sender")
    );
    let left = registry.pending_messages_for("recipient");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].message_id, "m2");
}

#[test]
fn messages_queued_under_one_retention_acknowledge_under_the_other() {
    let registry = sender_and_recipient();
    let _kept = queue_two(&registry, MessageRetention::All);
    // The earlier entries are keyed by message id, so a later per-sender one sits beside them rather than replacing.
    let _queued = registry.queue_direct(
        "m3".to_string(),
        "sender",
        "recipient",
        "2026-10-26T00:00:01Z".to_string(),
        serde_json::Value::Null,
        MessageRetention::LatestPerSender,
    );

    for message_id in ["m3", "m1", "m2"] {
        let (_acknowledged, _session, sender) = registry.acknowledge_message("recipient", message_id).unwrap();
        assert_eq!(sender, "sender");
    }
    assert!(registry.pending_messages_for("recipient").is_empty());
}

#[test]
fn retention_reads_its_kebab_case_names() {
    let parse = |name: &str| serde_json::from_value::<MessageRetention>(serde_json::json!(name)).unwrap();
    assert_eq!(parse("latest-per-sender"), MessageRetention::LatestPerSender);
    assert_eq!(parse("all"), MessageRetention::All);
}
