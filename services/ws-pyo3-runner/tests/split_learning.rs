//! End-to-end integration test: drive the split-learning demo's wire
//! protocol through `et-ws-pyo3-runner` hosting `split_learning_agent.py`.
//!
//! Layout: in-process et-ws-server (via `et-ws-test-server`), runner
//! subprocess as one agent, this test as the other agent. Control client
//! sends a synthetic `ACTIVATIONS_AND_LABELS` frame (base64-encoded JSON
//! envelope, the same shape `split_learning_demo/scripts/client.py`
//! produces); server default-broadcasts it to the runner; runner's
//! `handle_binary` calls into PyTorch, returns a `GRADS` frame; server
//! default-broadcasts that back; we assert it decodes cleanly with the
//! expected shape.
//!
//! Gated on `SPLIT_LEARNING_DEMO_SRC` so the test only runs when the
//! demo's `packages/split-learning-demo/src` tree is reachable on disk
//! (and the active Python env has torch / lightning / onnx). Set it to
//! enable the test:
//!
//!     SPLIT_LEARNING_DEMO_SRC=$PWD/split-learning-demo/packages/split-learning-demo/src \
//!         cargo test -p et-ws-pyo3-runner --test split_learning

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use edge_toolkit::ws::WsMessage;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite};

/// Mirror of `split_learning.schemas.message.WSMessage`. The demo serialises
/// `raw: dict[str, bytes]` by base64-encoding the bytes (see the
/// `json_encoders` config on the upstream pydantic model); we do the same
/// here so the runner sees byte-identical frames.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    type_: String,
    data: serde_json::Value,
    raw: BTreeMap<String, String>,
}

fn encode_frame(envelope: &Envelope) -> Vec<u8> {
    let json = serde_json::to_string(envelope).unwrap();
    BASE64.encode(json).into_bytes()
}

fn decode_frame(bytes: &[u8]) -> Envelope {
    let json_bytes = BASE64.decode(bytes).expect("outer base64");
    let json_text = String::from_utf8(json_bytes).expect("utf-8");
    serde_json::from_str(&json_text).expect("envelope json")
}

/// A small batch of float32 activations laid out exactly like the demo
/// client emits: shape `(batch, 16, 7, 7)` after the client's conv1 cut.
fn build_activations_frame() -> (Vec<u8>, Vec<i64>) {
    let shape = vec![2i64, 16, 7, 7];
    let n: usize = shape.iter().product::<i64>() as usize;
    let mut tensor_bytes = Vec::with_capacity(n * 4);
    for i in 0..n {
        // Deterministic-but-varied input; values centred around zero so
        // training doesn't explode in one step.
        let v = ((i as f32) * 0.001) - 0.5;
        tensor_bytes.extend_from_slice(&v.to_le_bytes());
    }

    // Labels: int64, batch=2 → 16 bytes.
    let labels_bytes: Vec<u8> = [3i64, 7i64].iter().flat_map(|n| n.to_le_bytes()).collect();

    let mut raw = BTreeMap::new();
    raw.insert("tensor".to_string(), BASE64.encode(&tensor_bytes));
    raw.insert("labels".to_string(), BASE64.encode(&labels_bytes));

    let envelope = Envelope {
        type_: "activations_and_labels".to_string(),
        data: serde_json::json!({ "tensor_shape": shape }),
        raw,
    };
    (encode_frame(&envelope), shape)
}

#[tokio::test(flavor = "current_thread")]
async fn split_learning_round_trip() {
    let demo_src = match std::env::var("SPLIT_LEARNING_DEMO_SRC") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SPLIT_LEARNING_DEMO_SRC not set; skipping split_learning_round_trip");
            return;
        }
    };

    let server = et_ws_test_server::start();

    // Control client registers first so we know the runner's broadcast
    // can target a peer.
    let (mut control, control_id) = control_client(&server.ws_url).await;
    eprintln!("control agent_id={control_id}");

    // Spawn the runner with the split-learning agent. PYO3_AGENT_PYTHONPATH
    // chains the agent's directory and the demo's `src/` so
    // `import split_learning` resolves.
    let agent_dir = format!(
        "{}/../ws-modules/pyo3-split-learning-mit/python",
        env!("CARGO_MANIFEST_DIR")
    );
    let pythonpath = format!("{agent_dir}:{demo_src}");
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("PYO3_AGENT_MODULE", "split_learning_agent")
        .env("PYO3_AGENT_PYTHONPATH", &pythonpath)
        .env("SPLIT_LEARNING_ACCELERATOR", "cpu")
        .env("WS_SERVER_URL", &server.ws_url)
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()),
        )
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn et-ws-pyo3-runner");

    let outcome = tokio::time::timeout(
        Duration::from_secs(60),
        round_trip(&mut control, &control_id),
    )
    .await;

    let _ = runner.kill();
    let _ = runner.wait();

    let observed = outcome.expect("round trip timed out").expect("round trip failed");
    assert_eq!(observed.type_, "grads", "expected GRADS reply");
    let loss = observed.data["loss"].as_f64().expect("loss is float");
    assert!(loss.is_finite() && loss >= 0.0, "loss={loss} (expected finite, non-negative)");
    let shape: Vec<i64> = observed.data["tensor_shape"]
        .as_array()
        .expect("shape is array")
        .iter()
        .map(|v| v.as_i64().expect("shape dim is i64"))
        .collect();
    // Server-side grads flow back to the client at the cut layer — same
    // shape as the activations we sent in.
    assert_eq!(shape, vec![2, 16, 7, 7], "grad tensor shape mismatch");
    eprintln!("split-learning round trip ok: loss={loss:.4} shape={shape:?}");
}

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

async fn round_trip(
    control: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    self_id: &str,
) -> Result<Envelope, String> {
    // Wait for the runner to register. Python init() spins up Lightning
    // Fabric + builds the CNN — give it generous time.
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let mut have_peer = false;
    while std::time::Instant::now() < deadline {
        let req = serde_json::to_string(&WsMessage::ListAgents).unwrap();
        control
            .send(tungstenite::Message::Text(req.into()))
            .await
            .map_err(|e| format!("send list_agents: {e}"))?;
        let poll_until = std::time::Instant::now() + Duration::from_millis(300);
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
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !have_peer {
        return Err("runner never registered".into());
    }

    // Send the synthetic ACTIVATIONS_AND_LABELS frame as a binary frame
    // (server doesn't recognise the demo's envelope, so it default-
    // broadcasts the bytes verbatim to the runner peer).
    let (frame, _shape) = build_activations_frame();
    control
        .send(tungstenite::Message::Binary(frame.into()))
        .await
        .map_err(|e| format!("send activations frame: {e}"))?;

    // Drain until we see the runner's GRADS reply (another binary frame).
    // Ignore typed et-* messages — those are status / list / ack noise.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => return Ok(decode_frame(&bytes)),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(format!("recv: {e}")),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for GRADS".into()),
        }
    }
    Err("deadline exceeded".into())
}
