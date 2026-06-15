//! Verify the multi-send path: one inbound binary frame results in N
//! outbound binary frames pushed via `WsSender.binary(...)`, with no
//! reply-by-return. Test client sends a single byte `count` and asserts
//! it receives exactly `count` distinct one-byte echoes.

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::indexing_slicing,
    clippy::single_call_fn,
    reason = "integration test: Instant/Duration poll-loop math, small counts/indexes, single-use helpers"
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
async fn fanout_module_emits_multiple_frames() -> Result<(), Box<dyn Error>> {
    let server = et_ws_test_server::start();
    let (mut control, control_id) = control_client(&server.ws_url).await?;

    let python_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("RUNNER_MODULE", "fanout")
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
    let expected: Vec<u8> = (0u8..5).collect();
    if observed != expected {
        return Err(format!("received {observed:?}, expected {expected:?}").into());
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

    // Ask for 5 frames back.
    let count: u8 = 5;
    control.send(tungstenite::Message::Binary(vec![count])).await?;

    // Collect exactly `count` binary frames. Ignore typed et-* envelopes.
    let mut received = Vec::with_capacity(count as usize);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while received.len() < count as usize && std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => {
                if bytes.len() != 1 {
                    return Err(format!("fanout produced {}-byte frame, expected 1", bytes.len()).into());
                }
                received.push(bytes[0]);
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => return Err(format!("recv: {e}").into()),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for fan-out frames".into()),
        }
    }
    if received.len() != count as usize {
        return Err(format!("got {} frames, expected {count}", received.len()).into());
    }
    Ok(received)
}
