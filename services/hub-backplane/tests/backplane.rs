//! The NATS backplane against a real broker: directory, mailbox, delivery fabric, config parsing and failure reports.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use edge_toolkit::ws::AgentConnectionState;
use edge_toolkit::ws_server::PendingDirectMessage;
use et_hub_backplane::{Backplane, BackplaneConfig, BackplaneError, Frame, Inbound, NatsBackplane, Presence};
use et_test_nats::NatsServer;
use futures_util::{Stream, StreamExt as _};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn pending(message_id: &str, from: &str) -> PendingDirectMessage {
    PendingDirectMessage::new(
        message_id.to_owned(),
        from.to_owned(),
        "2026-10-19T00:00:00Z".to_owned(),
        serde_json::json!({"n": message_id}),
    )
}

/// The next inbound frame, failing the test if none arrives within five seconds.
async fn next_inbound(stream: &mut (impl Stream<Item = Inbound> + Unpin)) -> Inbound {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
}

/// A raw client and the KV bucket `bucket`, for writing records the backplane itself never would.
async fn raw_bucket(server: &NatsServer, bucket: &str) -> async_nats::jetstream::kv::Store {
    let client = async_nats::connect(server.url()).await.unwrap();
    async_nats::jetstream::new(client).get_key_value(bucket).await.unwrap()
}

#[test]
fn config_names_local_or_a_nats_url() {
    assert_eq!("local".parse::<BackplaneConfig>().unwrap(), BackplaneConfig::Local);
    assert_eq!(
        "nats://h:4222".parse::<BackplaneConfig>().unwrap(),
        BackplaneConfig::Nats("nats://h:4222".to_owned())
    );
    assert_eq!(
        "tls://h:4222".parse::<BackplaneConfig>().unwrap(),
        BackplaneConfig::Nats("tls://h:4222".to_owned())
    );
    assert!(matches!(
        "redis://h".parse::<BackplaneConfig>(),
        Err(BackplaneError::UnrecognisedConfig(value)) if value == "redis://h"
    ));
    assert_eq!(BackplaneConfig::default(), BackplaneConfig::Local);

    let parsed: BackplaneConfig = serde_json::from_str(r#""nats://h:1""#).unwrap();
    assert_eq!(parsed, BackplaneConfig::Nats("nats://h:1".to_owned()));
    let error = serde_json::from_str::<BackplaneConfig>(r#""bogus""#).unwrap_err();
    assert!(error.to_string().contains("bogus"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_backplane_knows_nobody() {
    let backplane = Backplane::connect(&BackplaneConfig::Local).await.unwrap();
    assert!(matches!(backplane, Backplane::Local));
    assert!(!backplane.knows("anyone").await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_broker_fails_to_connect_naming_its_url() {
    let url = format!("nats://127.0.0.1:{}", et_test_helpers::reserve_port());
    let error = Backplane::connect(&BackplaneConfig::Nats(url.clone()))
        .await
        .unwrap_err();
    assert!(matches!(&error, BackplaneError::Connect { url: named, .. } if *named == url));
    assert!(error.to_string().contains(&url));
}

#[test]
fn credentials_never_reach_debug_output_or_error_text() {
    // A server list, as NATS takes one; joined here so the link checker sees each URL whole rather than one unparsable.
    let servers = ["nats://hub:s3cret@a:4222", "tls://b:4223/path"].join(",");
    let shown = ["nats://***@a:4222", "tls://b:4223/path"].join(",");
    let config = BackplaneConfig::Nats(servers);
    assert_eq!(format!("{config:?}"), format!("Nats({shown:?})"));
    assert_eq!(format!("{:?}", BackplaneConfig::Local), "Local");

    let error = "redis://hub:s3cret@a".parse::<BackplaneConfig>().unwrap_err();
    assert_eq!(
        error.to_string(),
        r#"backplane "redis://***@a" is neither `local` nor a nats:// or tls:// URL"#
    );
    let schemeless = "hub:s3cret@a".parse::<BackplaneConfig>().unwrap_err();
    assert_eq!(
        schemeless.to_string(),
        r#"backplane "***@a" is neither `local` nor a nats:// or tls:// URL"#
    );
    let plain = "redis://a".parse::<BackplaneConfig>().unwrap_err();
    assert_eq!(
        plain.to_string(),
        r#"backplane "redis://a" is neither `local` nor a nats:// or tls:// URL"#
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_connection_names_the_server_but_not_its_credentials() {
    let port = et_test_helpers::reserve_port();
    let url = format!("nats://hub:s3cret@127.0.0.1:{port}");
    let error = Backplane::connect(&BackplaneConfig::Nats(url)).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("failed to connect to NATS at nats://***@127.0.0.1:{port}")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_directory_follows_an_agent_across_hubs() {
    let server = NatsServer::start();
    let first = NatsBackplane::connect(&server.url()).await.unwrap();
    let second = NatsBackplane::connect(&server.url()).await.unwrap();
    assert_ne!(first.hub_id(), second.hub_id());

    assert_eq!(first.presence("a.b").await.unwrap(), Presence::Unknown);
    first.announce("a.b", LOOPBACK).await.unwrap();
    let on_first = Presence::Connected {
        hub_id: first.hub_id().to_owned(),
    };
    assert_eq!(second.presence("a.b").await.unwrap(), on_first);

    // The agent reconnects to the second hub; the first hub's late close must leave it there.
    second.announce("a.b", LOOPBACK).await.unwrap();
    first.withdraw("a.b").await.unwrap();
    let on_second = Presence::Connected {
        hub_id: second.hub_id().to_owned(),
    };
    assert_eq!(first.presence("a.b").await.unwrap(), on_second);

    second.withdraw("a.b").await.unwrap();
    assert_eq!(first.presence("a.b").await.unwrap(), Presence::Disconnected);
    // Withdrawing again, or withdrawing an agent nobody registered, changes nothing.
    second.withdraw("a.b").await.unwrap();
    second.withdraw("ghost").await.unwrap();
    assert_eq!(first.presence("ghost").await.unwrap(), Presence::Unknown);

    first.announce("z", LOOPBACK).await.unwrap();
    let listed = second.list_agents().await.unwrap();
    let ids = listed.iter().map(|agent| agent.agent_id.as_str()).collect::<Vec<_>>();
    assert_eq!(ids, ["a.b", "z"]);
    assert_eq!(listed[0].state, AgentConnectionState::Disconnected);
    assert_eq!(listed[1].state, AgentConnectionState::Connected);

    let backplane = Backplane::Nats(Box::new(first));
    assert!(backplane.knows("z").await.unwrap());
    assert!(!backplane.knows("ghost").await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_directory_entry_reads_as_unknown() {
    let server = NatsServer::start();
    let hub = NatsBackplane::connect(&server.url()).await.unwrap();
    hub.announce("gone", LOOPBACK).await.unwrap();
    raw_bucket(&server, "et-agents")
        .await
        .delete(const_hex::encode("gone"))
        .await
        .unwrap();
    assert_eq!(hub.presence("gone").await.unwrap(), Presence::Unknown);
    assert_eq!(hub.list_agents().await.unwrap(), []);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_records_are_reported_not_ignored() {
    let server = NatsServer::start();
    let hub = NatsBackplane::connect(&server.url()).await.unwrap();
    let agents = raw_bucket(&server, "et-agents").await;
    let _revision = agents
        .put(const_hex::encode("bad"), Bytes::from_static(b"{"))
        .await
        .unwrap();
    assert!(matches!(hub.presence("bad").await, Err(BackplaneError::Codec(_))));
    assert!(matches!(hub.list_agents().await, Err(BackplaneError::Codec(_))));
    agents.delete(const_hex::encode("bad")).await.unwrap();

    let _revision = agents.put("zz", Bytes::from_static(b"{}")).await.unwrap();
    assert!(matches!(hub.list_agents().await, Err(BackplaneError::Key(_))));
    agents.delete("zz").await.unwrap();
    let _revision = agents
        .put(const_hex::encode([0xff]), Bytes::from_static(b"{}"))
        .await
        .unwrap();
    assert!(matches!(hub.list_agents().await, Err(BackplaneError::InvalidText(_))));

    let mailboxes = raw_bucket(&server, "et-pending").await;
    let _revision = mailboxes
        .put(const_hex::encode("bad"), Bytes::from_static(b"["))
        .await
        .unwrap();
    assert!(matches!(hub.pending_for("bad").await, Err(BackplaneError::Codec(_))));
    assert!(matches!(
        hub.queue("bad", pending("m", "x")).await,
        Err(BackplaneError::Codec(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mailbox_keeps_the_newest_message_per_sender_until_acknowledged() {
    let server = NatsServer::start();
    let first = NatsBackplane::connect(&server.url()).await.unwrap();
    let second = NatsBackplane::connect(&server.url()).await.unwrap();

    assert!(first.pending_for("r").await.unwrap().is_empty());
    first.queue("r", pending("m1", "alice")).await.unwrap();
    first.queue("r", pending("m2", "alice")).await.unwrap();
    second.queue("r", pending("m3", "bob")).await.unwrap();
    let ids = |messages: Vec<PendingDirectMessage>| {
        messages
            .into_iter()
            .map(|message| message.message_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(second.pending_for("r").await.unwrap()), ["m2", "m3"]);

    assert_eq!(second.acknowledge("r", "m2").await.unwrap(), "alice");
    assert!(matches!(
        first.acknowledge("r", "m2").await,
        Err(BackplaneError::NoPendingMessage(id)) if id == "m2"
    ));
    assert_eq!(ids(first.pending_for("r").await.unwrap()), ["m3"]);

    // A mailbox someone deleted outright starts again from empty.
    raw_bucket(&server, "et-pending")
        .await
        .delete(const_hex::encode("r"))
        .await
        .unwrap();
    assert!(first.pending_for("r").await.unwrap().is_empty());
    first.queue("r", pending("m4", "carol")).await.unwrap();
    assert_eq!(ids(first.pending_for("r").await.unwrap()), ["m4"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_senders_all_land_in_one_mailbox() {
    let server = NatsServer::start();
    let hubs = [
        NatsBackplane::connect(&server.url()).await.unwrap(),
        NatsBackplane::connect(&server.url()).await.unwrap(),
    ];
    // Every writer is spawned before any is awaited, so all forty race for the same mailbox revision.
    let mut writers = Vec::with_capacity(40);
    for (index, hub) in (0..40_u32).zip(hubs.iter().cycle()) {
        let hub = hub.clone();
        writers.push(tokio::spawn(async move {
            hub.queue("busy", pending(&format!("m{index:02}"), &format!("s{index:02}")))
                .await
        }));
    }
    for writer in writers {
        writer.await.unwrap().unwrap();
    }
    assert_eq!(hubs[0].pending_for("busy").await.unwrap().len(), 40);
}

// Fixed in `NatsBackplane::subscribe`, which now returns only once the server has registered its subscriptions.
// Before that, on slow runners (the `override (mingw)` lane of test.yaml) this timed out in `next_inbound` with
//
//     panicked at services\hub-backplane\tests\backplane.rs:30:10:
//     called `Result::unwrap()` on an `Err` value: Elapsed(())
//
// on commit https://github.com/edge-toolkit/core/commit/c91d332c72d0c94f9fd4aa915c1d485fee251fd0 at
// https://github.com/edge-toolkit/core/actions/runs/39857810014/job/120100468951.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_reach_the_named_hub_and_broadcasts_reach_the_others() {
    let server = NatsServer::start();
    let first = NatsBackplane::connect(&server.url()).await.unwrap();
    let second = NatsBackplane::connect(&server.url()).await.unwrap();
    let mut first_inbox = Box::pin(first.subscribe().await.unwrap());
    let mut second_inbox = Box::pin(second.subscribe().await.unwrap());

    let text = Frame::Text("hello".to_owned());
    first.send_to_hub(second.hub_id(), "b", text.clone()).await.unwrap();
    assert_eq!(
        next_inbound(&mut second_inbox).await,
        Inbound::Direct {
            to_agent_id: "b".to_owned(),
            frame: text
        }
    );

    let binary = Frame::Binary(Bytes::from_static(&[0, 159, 255]));
    first.broadcast("a", binary.clone()).await.unwrap();
    second.broadcast("b", Frame::Text("from b".to_owned())).await.unwrap();
    assert_eq!(
        next_inbound(&mut second_inbox).await,
        Inbound::Broadcast {
            from_agent_id: "a".to_owned(),
            frame: binary
        }
    );
    // The first hub hears the second's broadcast, and never its own.
    assert_eq!(
        next_inbound(&mut first_inbox).await,
        Inbound::Broadcast {
            from_agent_id: "b".to_owned(),
            frame: Frame::Text("from b".to_owned())
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undecodable_messages_are_skipped_and_the_stream_carries_on() {
    let server = NatsServer::start();
    let hub = NatsBackplane::connect(&server.url()).await.unwrap();
    let mut inbox = Box::pin(hub.subscribe().await.unwrap());
    let raw = async_nats::connect(server.url()).await.unwrap();
    let subject = format!("et.hub.{}", hub.hub_id());

    let malformed = [
        (vec![("Et-Kind", "text")], Bytes::from_static(b"no target")),
        (vec![("Et-To", "b")], Bytes::from_static(b"no kind")),
        (
            vec![("Et-To", "b"), ("Et-Kind", "morse")],
            Bytes::from_static(b"unknown kind"),
        ),
        (
            vec![("Et-To", "b"), ("Et-Kind", "text")],
            Bytes::from_static(&[0xff, 0xfe]),
        ),
    ];
    for (pairs, body) in malformed {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(name, value);
        }
        raw.publish_with_headers(subject.clone(), headers, body).await.unwrap();
    }
    raw.publish("et.broadcast", Bytes::from_static(b"headerless"))
        .await
        .unwrap();
    hub.send_to_hub(hub.hub_id(), "b", Frame::Text("valid".to_owned()))
        .await
        .unwrap();

    assert_eq!(
        next_inbound(&mut inbox).await,
        Inbound::Direct {
            to_agent_id: "b".to_owned(),
            frame: Frame::Text("valid".to_owned())
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_broker_surfaces_as_an_error_naming_the_action() {
    let server = NatsServer::start();
    let hub = NatsBackplane::connect(&server.url()).await.unwrap();
    drop(server);

    let error = hub.announce("a", LOOPBACK).await.unwrap_err();
    assert!(matches!(
        &error,
        BackplaneError::Nats {
            action: "announce an agent",
            ..
        }
    ));
    assert_eq!(error.to_string(), "NATS failed to announce an agent");
}
