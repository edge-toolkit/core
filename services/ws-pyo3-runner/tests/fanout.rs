//! Verify the multi-send path: one inbound binary frame results in N
//! outbound binary frames pushed via `WsSender.binary(...)`, with no
//! reply-by-return. Test client sends a single byte `count` and asserts
//! it receives exactly `count` distinct one-byte echoes.

use std::process::{Command, Stdio};
use std::time::Duration;

use edge_toolkit::ws::WsMessage;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite};

async fn control_client(
    ws_url: &str,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    let (mut socket, _) = connect_async(ws_url).await.expect("control connect");
    let connect = serde_json::to_string(&WsMessage::Connect { agent_id: None }).unwrap();
    socket.send(tungstenite::Message::Text(connect.into())).await.unwrap();
    loop {
        let frame = socket.next().await.expect("recv").expect("recv ok");
        if let tungstenite::Message::Text(text) = frame
            && let Ok(WsMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<WsMessage>(&text)
        {
            return (socket, agent_id);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_module_emits_multiple_frames() {
    let server = et_ws_test_server::start();
    let (mut control, control_id) = control_client(&server.ws_url).await;

    let python_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("PYO3_AGENT_MODULE", "fanout")
        .env("PYO3_AGENT_PYTHONPATH", &python_path)
        .env("WS_SERVER_URL", &server.ws_url)
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()),
        )
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn et-ws-pyo3-runner");

    let outcome = tokio::time::timeout(Duration::from_secs(30), exercise(&mut control, &control_id)).await;
    let _ = runner.kill();
    let _ = runner.wait();

    let observed = outcome.expect("fanout timed out").expect("fanout failed");
    let expected: Vec<u8> = (0u8..5).collect();
    assert_eq!(observed, expected, "received frames do not match expected sequence");
}

async fn exercise(
    control: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    self_id: &str,
) -> Result<Vec<u8>, String> {
    // Wait for the runner to register so our broadcast has a peer.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut have_peer = false;
    while std::time::Instant::now() < deadline {
        let req = serde_json::to_string(&WsMessage::ListAgents).unwrap();
        control
            .send(tungstenite::Message::Text(req.into()))
            .await
            .map_err(|e| format!("send list_agents: {e}"))?;
        let poll_until = std::time::Instant::now() + Duration::from_millis(250);
        while std::time::Instant::now() < poll_until {
            let remaining = poll_until - std::time::Instant::now();
            match tokio::time::timeout(remaining, control.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                    if let Ok(WsMessage::ListAgentsResponse { agents }) = serde_json::from_str::<WsMessage>(&text)
                        && agents.iter().any(|a| a.agent_id != self_id)
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
    control
        .send(tungstenite::Message::Binary(vec![count].into()))
        .await
        .map_err(|e| format!("send request: {e}"))?;

    // Collect exactly `count` binary frames. Ignore typed et-* envelopes.
    let mut received = Vec::with_capacity(count as usize);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while received.len() < count as usize && std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => {
                assert_eq!(bytes.len(), 1, "fanout produced unexpected frame size");
                received.push(bytes[0]);
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(format!("recv: {e}")),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for fan-out frames".into()),
        }
    }
    if received.len() != count as usize {
        return Err(format!("got {} frames, expected {}", received.len(), count));
    }
    Ok(received)
}
