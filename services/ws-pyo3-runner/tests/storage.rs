//! Verify the `WsStorage` round-trip end to end: Python writes a blob
//! via `storage.put` and reads it back via `storage.get`, both going
//! through the runner's `/storage` HTTP client and the test server's
//! storage service. Confirms the full path (4-space indent = code block):
//!
//!     Python -> WsStorage::put -> mpsc::StorageOp::Put -> storage_worker
//!       -> et_rest_client put_file -> et-storage-service::agent_put_file
//!       -> disk (under TempDir)
//!       -> et_rest_client get_file -> bytes -> oneshot -> Python

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::single_call_fn,
    reason = "integration test: Instant/Duration poll-loop math and single-use helpers"
)]

use std::error::Error;
use std::process::{Command, Stdio};
use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::{connect_async, tungstenite};

type ControlSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn control_client(ws_url: &str) -> Result<(ControlSocket, String), Box<dyn Error>> {
    let (mut socket, _) = connect_async(ws_url).await?;
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id: None })?;
    socket.send(tungstenite::Message::Text(connect)).await?;
    loop {
        let Some(frame) = socket.next().await else {
            return Err("control socket closed before connect-ack".into());
        };
        if let tungstenite::Message::Text(text) = frame?
            && let Ok(ServerMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<ServerMessage>(&text)
        {
            return Ok((socket, agent_id));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn storage_put_then_get_round_trip() -> Result<(), Box<dyn Error>> {
    let server = et_ws_test_server::start();
    let (mut control, control_id) = control_client(&server.ws_url).await?;

    let python_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("RUNNER_MODULE", "storage_pingpong")
        .env("PYO3_PYTHONPATH", &python_path)
        .env("WS_SERVER_URL", &server.ws_url)
        .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    let outcome = tokio::time::timeout(Duration::from_secs(30), exercise(&mut control, &control_id)).await;
    drop(runner.kill());
    drop(runner.wait());

    let observed = outcome??;
    let expected: &[u8] = b"a quick brown fox jumps over the lazy dog";
    if observed.as_slice() != expected {
        return Err(format!("stored bytes do not match: {observed:?}").into());
    }
    Ok(())
}

async fn exercise(control: &mut ControlSocket, self_id: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    // Wait for the runner to register so our broadcast has a peer.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut have_peer = false;
    while std::time::Instant::now() < deadline {
        let req = serde_json::to_string(&ClientMessage::ListAgents)?;
        control.send(tungstenite::Message::Text(req)).await?;
        let poll_until = std::time::Instant::now() + Duration::from_millis(250);
        while std::time::Instant::now() < poll_until {
            let remaining = poll_until - std::time::Instant::now();
            match tokio::time::timeout(remaining, control.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                    if let Ok(ServerMessage::ListAgentsResponse { agents }) =
                        serde_json::from_str::<ServerMessage>(&text)
                        && agents.iter().any(|summary| summary.agent_id != self_id)
                    {
                        have_peer = true;
                        break;
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => break,
            }
        }
        if have_peer {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !have_peer {
        return Err("runner never registered".into());
    }

    let key = "hello.txt";
    let value: &[u8] = b"a quick brown fox jumps over the lazy dog";

    // PUT: send `key\x00value` -- the module's `on_binary_frame` splits
    // on the NUL and calls storage.put.
    let mut put_frame = Vec::with_capacity(key.len() + 1 + value.len());
    put_frame.extend_from_slice(key.as_bytes());
    put_frame.push(0);
    put_frame.extend_from_slice(value);
    control.send(tungstenite::Message::Binary(put_frame)).await?;

    // Give the storage worker a moment to PUT to disk before we GET.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // GET: send `key` only -- the module's `on_binary_frame` treats
    // a NUL-free frame as a get and pushes the result back via
    // `send.binary(...)`.
    control
        .send(tungstenite::Message::Binary(key.as_bytes().to_vec()))
        .await?;

    // Drain until we see the runner's binary reply.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => return Ok(bytes),
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => return Err(format!("recv: {e}").into()),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for storage reply".into()),
        }
    }
    Err("deadline exceeded".into())
}
