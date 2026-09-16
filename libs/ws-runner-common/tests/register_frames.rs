//! Covers what the registration handshake does with frames that are not the ack it is waiting for.
//!
//! `register_once` reads from the socket until an `et-connect-ack` arrives, and the two ways that read can go
//! wrong are both silent from the runner's point of view. A binary frame arriving first -- a broadcast from
//! another agent, which the hub forwards verbatim to every connected client -- must be stepped over rather
//! than ending the handshake; treating it as a failure would make registration fail whenever a peer happened
//! to be chatty. A socket that closes before acking must come back as `ConnectionClosed` rather than hanging
//! until the budget expires, so the retry loop can report what actually happened.
//!
//! Both cases need a hub that misbehaves on purpose, so each test stands one up rather than driving the real
//! server, which has no way to be asked for either shape. Accepting a connection is the half of the websocket
//! protocol the crate itself never uses -- the runner only ever dials out -- so the server side arrives as a
//! test-only dependency feature.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::{ConnectStatus, ServerMessage};
use et_ws_runner_common::{ConnectError, connect_and_register};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// The ack frame a real hub answers `et-connect` with.
///
/// Built through the wire enum rather than hand-written JSON, so a rename of the type tag breaks this test
/// instead of quietly making it test nothing.
#[expect(
    clippy::single_call_fn,
    reason = "the wire fixture for the stub hub; named so the handshake body reads as a sequence of frames"
)]
fn ack_frame(agent_id: &str) -> String {
    serde_json::to_string(&ServerMessage::ConnectAck {
        agent_id: agent_id.to_owned(),
        status: ConnectStatus::Assigned,
    })
    .unwrap()
}

#[tokio::test]
async fn a_binary_frame_before_the_ack_is_stepped_over() {
    let port = et_test_helpers::reserve_port();
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let hub = tokio::spawn(async move {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _connect = socket.next().await.unwrap().unwrap();
        // A binary frame is the hub relaying another agent's payload; it carries no handshake meaning.
        socket.send(Message::binary(vec![0_u8, 1, 2])).await.unwrap();
        socket.send(Message::text(ack_frame("agent-7"))).await.unwrap();
        // Hold the socket open until the client has taken the ack and returned it to the caller.
        tokio::time::sleep(Duration::from_millis(250)).await;
    });

    let url = format!("ws://127.0.0.1:{port}/ws");
    // Unwrapped rather than matched: a failure here *is* the regression this test exists for -- the binary
    // frame ended the handshake -- and the panic carries the error that says so.
    let (_socket, agent_id, status) = connect_and_register(&url, None, Some(Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(agent_id, "agent-7");
    assert_eq!(status, ConnectStatus::Assigned);
    hub.await.unwrap();
}

#[tokio::test]
async fn a_socket_closed_before_the_ack_reports_connection_closed() {
    let port = et_test_helpers::reserve_port();
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let hub = tokio::spawn(async move {
        // Every attempt gets the same treatment, because the caller retries: read the registration, then hang
        // up without answering. A hub restarting mid-handshake looks exactly like this from the outside.
        loop {
            let Ok((stream, _addr)) = listener.accept().await else {
                return;
            };
            let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                continue;
            };
            let _connect = socket.next().await;
            let _closed = socket.close(None).await;
            // Drain until the client's close reply has been read and the stream ends, so the socket is
            // dropped only after the close handshake completes. Dropping it straight after sending the close
            // frame leaves that reply unread in the receive buffer, which makes the kernel tear the connection
            // down with a reset instead of a FIN; Linux still hands the client its end-of-stream, but Windows
            // surfaces the reset first, so the client saw
            // `WebSocket(Io(Os { code: 10053, kind: ConnectionAborted, ... }))` instead of `ConnectionClosed`
            // on the windows-11-arm lane at commit
            // https://github.com/edge-toolkit/core/commit/c6c4fce73dd25aa58754963867ccf9523caae1bb
            // (<https://github.com/edge-toolkit/core/actions/runs/35045764412/job/104635143335>).
            while let Some(Ok(_frame)) = socket.next().await {}
        }
    });

    let url = format!("ws://127.0.0.1:{port}/ws");
    let err = connect_and_register(&url, None, Some(Duration::from_millis(400)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ConnectError::ConnectionClosed),
        "expected the closed-socket error rather than a timeout, got {err:?}"
    );
    hub.abort();
}
