//! Smoke test: spin up an in-process et-ws-server, launch et-ws-pyo3-runner
//! with the bundled `echo.py`, and verify it (a) successfully registers as
//! an agent and (b) echoes a frame we broadcast back to us.
//!
//! Two clients connect: a control client (this test, talking
//! `tokio-tungstenite` directly) and the pyo3 runner (subprocess). The
//! control client broadcasts an unrecognised text frame; et-ws-server
//! default-broadcasts it to the runner; the echo module returns it; the
//! server default-broadcasts the runner's reply back to the control
//! client. Round-trip proves both the protocol alignment and the Python
//! dispatch.

use std::process::{Command, Stdio};
use std::time::Duration;

use edge_toolkit::ws::WsMessage;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite};

/// Open a control client and drive et-connect until we have an agent_id.
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
        let frame = socket.next().await.expect("control recv").expect("control recv ok");
        let tungstenite::Message::Text(text) = frame else {
            continue;
        };
        if let Ok(WsMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<WsMessage>(&text) {
            return (socket, agent_id);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn echo_module_round_trips() {
    let server = et_ws_test_server::start();

    // Stage 1: control client registers first so the runner has a peer to
    // broadcast back to.
    let (mut control, control_id) = control_client(&server.ws_url).await;
    eprintln!("control agent_id={control_id}");

    // Stage 2: spawn the runner subprocess. `manifest_dir/python` holds
    // the echo module.
    let echo_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("PYO3_AGENT_MODULE", "echo")
        .env("PYO3_AGENT_PYTHONPATH", &echo_path)
        .env("WS_SERVER_URL", &server.ws_url)
        // Silence the runner's logs unless the test is invoked with --nocapture
        // and the operator opted in via RUST_LOG.
        .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn et-ws-pyo3-runner");

    // Stage 3: wait until the runner shows up in the registry, then send
    // an arbitrary text frame and assert we get the same string back via
    // the default broadcast path.
    let payload = r#"{"hello":"world","from":"control"}"#;
    let echo_result =
        tokio::time::timeout(Duration::from_secs(20), echo_round_trip(&mut control, payload, &control_id)).await;

    let _ = runner.kill();
    let _ = runner.wait();

    let observed = echo_result.expect("echo round-trip timed out").expect("echo failed");
    assert_eq!(observed, payload, "echo response did not match");
}

/// Poll list_agents until we see at least one peer (the runner), then
/// broadcast a frame and wait for it to land back on the control socket.
async fn echo_round_trip(
    control: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    payload: &str,
    self_id: &str,
) -> Result<String, String> {
    // Wait for the runner to register. The test server has no shared
    // handle into the registry, so we poll `et-list-agents` until a peer
    // shows up. The runner needs ~1s to spawn + init Python + connect,
    // so give it a generous deadline.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut have_peer = false;
    while std::time::Instant::now() < deadline {
        let req = serde_json::to_string(&WsMessage::ListAgents).unwrap();
        control
            .send(tungstenite::Message::Text(req.into()))
            .await
            .map_err(|e| format!("send list_agents: {e}"))?;
        // Drain everything available for 250ms — there may be multiple
        // queued responses from earlier polls.
        let poll_deadline = std::time::Instant::now() + Duration::from_millis(250);
        while std::time::Instant::now() < poll_deadline {
            let remaining = poll_deadline - std::time::Instant::now();
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

    // Default-broadcast (frame the server doesn't recognise as a typed
    // et-* message gets fanned out to other agents as-is).
    control
        .send(tungstenite::Message::Text(payload.to_string().into()))
        .await
        .map_err(|e| format!("send payload: {e}"))?;

    // The runner echoes back a Text frame containing the same payload.
    // Drain frames until we see it; ignore et-* protocol noise and our
    // own list_agents_response loops still in flight.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                if serde_json::from_str::<WsMessage>(&text).is_ok() {
                    // typed et-* envelope (status / list / ack), keep draining
                    continue;
                }
                if text == payload {
                    return Ok(text.to_string());
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(format!("recv error: {e}")),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for echo".into()),
        }
    }
    Err("deadline exceeded".into())
}
