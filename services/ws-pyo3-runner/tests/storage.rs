//! Verify the `WsStorage` round-trip end to end: Python writes a blob
//! via `storage.put` and reads it back via `storage.get`, both going
//! through the runner's `/storage` HTTP client and the test server's
//! storage service. Confirms the full path:
//!
//!   Python → WsStorage::put → mpsc::StorageOp::Put → storage_worker
//!     → reqwest::Client::put → et-storage-service::agent_put_file
//!     → disk (under TempDir)
//!     → reqwest::Client::get → bytes → oneshot → Python

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
async fn storage_put_then_get_round_trip() {
    let server = et_ws_test_server::start();
    let (mut control, control_id) = control_client(&server.ws_url).await;

    let python_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("PYO3_AGENT_MODULE", "storage_pingpong")
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

    let observed = outcome.expect("storage timed out").expect("storage failed");
    let expected = b"a quick brown fox jumps over the lazy dog";
    assert_eq!(observed.as_slice(), expected, "stored bytes do not match");
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

    let key = "hello.txt";
    let value: &[u8] = b"a quick brown fox jumps over the lazy dog";

    // PUT: send `key\x00value` — the module's `on_binary_frame` splits
    // on the NUL and calls storage.put.
    let mut put_frame = Vec::with_capacity(key.len() + 1 + value.len());
    put_frame.extend_from_slice(key.as_bytes());
    put_frame.push(0);
    put_frame.extend_from_slice(value);
    control
        .send(tungstenite::Message::Binary(put_frame.into()))
        .await
        .map_err(|e| format!("send put frame: {e}"))?;

    // Give the storage worker a moment to PUT to disk before we GET.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // GET: send `key` only — the module's `on_binary_frame` treats
    // a NUL-free frame as a get and pushes the result back via
    // `send.binary(...)`.
    control
        .send(tungstenite::Message::Binary(key.as_bytes().to_vec().into()))
        .await
        .map_err(|e| format!("send get frame: {e}"))?;

    // Drain until we see the runner's binary reply.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => return Ok(bytes.to_vec()),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(format!("recv: {e}")),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for storage reply".into()),
        }
    }
    Err("deadline exceeded".into())
}
