//! The fake-agent side of the math1 storage exchange, shared by every runner's math1 test.
//!
//! The math1 family's protocol: a fake agent injects the canonical input JSON (committed at
//! `data/math1-input.json`) into the ws-server's storage, then broadcasts a pointer frame
//! `{"type":"math1-input","bucket":...,"filename":...}` over the hub (an unrecognised frame the
//! server relays verbatim to every other agent). The math1 module under test reads the input from
//! storage, runs the `FedAvg` kernel with the file's parameters, and writes its global model to
//! `math1-output.json` in its own bucket, where [`drive_math1_exchange`] picks it up and
//! [`verify_math1_model`] checks it against the expected weights for the canonical input. The
//! pointer is re-broadcast until the output lands, so a module that connects after the first
//! broadcast still hears it.

#![expect(
    clippy::float_arithmetic,
    reason = "verifying FedAvg model floats against the expected constants is float math by design"
)]

use std::path::Path;
use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Storage object name of the canonical input, inside the fake agent's bucket.
pub const MATH1_INPUT_FILENAME: &str = "math1-input.json";
/// Storage object name each math1 module writes its global model to, inside its own bucket.
pub const MATH1_OUTPUT_FILENAME: &str = "math1-output.json";
/// The canonical input dataset + hyperparameters, committed so every module computes the same run.
pub const MATH1_INPUT_JSON: &str = include_str!("../data/math1-input.json");
/// Expected global model for the canonical input; the one hard-coded verification point.
pub const MATH1_EXPECTED_WEIGHT: f64 = 2.027_406_278_700_665;
/// Expected bias for the canonical input; see [`MATH1_EXPECTED_WEIGHT`].
pub const MATH1_EXPECTED_BIAS: f64 = 0.914_969_140_718_165_6;
/// Comparison tolerance: effectively exact for f64s that survived a JSON round-trip.
pub const MATH1_TOLERANCE: f64 = 1e-12;

/// How often the pointer is re-broadcast and the output file re-polled.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Failure of the math1 exchange, either in the fake agent's transport or in the module's output.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Math1Error {
    /// Websocket transport failure on the fake agent's connection.
    ///
    /// Boxed: tungstenite's error is large, and `result_large_err` fires on every `Result`
    /// carrying it inline.
    #[error(transparent)]
    Transport(Box<tokio_tungstenite::tungstenite::Error>),
    /// The module's stored output was not the expected JSON shape.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Protocol-level failure: a timeout, a missing field, or a wrong model value.
    #[error("{0}")]
    Protocol(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for Math1Error {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Transport(Box::new(err))
    }
}

/// Connect the fake agent, inject the input, broadcast the pointer, and await the module's output.
///
/// Returns the (weight, bias) parsed from the module's `math1-output.json`. The input file is
/// written straight into `storage_dir` under the fake agent's bucket (the disk layout the local
/// storage backend serves), and the module's output is read back the same way -- the module itself
/// exercises the real REST/keyvalue path in both directions.
pub async fn drive_math1_exchange(
    ws_url: &str,
    storage_dir: &Path,
    budget: Duration,
) -> Result<(f64, f64), Math1Error> {
    let (mut socket, _response) = connect_async(ws_url).await?;
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id: None })?;
    socket.send(Message::Text(connect)).await?;

    let deadline = tokio::time::Instant::now() + budget;
    let mut fake_id = String::new();
    let mut peers: Vec<String> = Vec::new();
    let mut pointer = String::new();

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(Math1Error::Protocol(format!(
                "math1 exchange timed out: fake_id={fake_id:?} peers={peers:?} (no {MATH1_OUTPUT_FILENAME} yet)"
            )));
        }

        // Drain inbound frames for one poll interval, tracking the ack and the agent roster.
        let drain_until = tokio::time::Instant::now() + POLL_INTERVAL;
        while tokio::time::Instant::now() < drain_until {
            let remaining = drain_until - tokio::time::Instant::now();
            match tokio::time::timeout(remaining, socket.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(ServerMessage::ConnectAck { agent_id, .. }) => {
                        fake_id = agent_id;
                        let bucket_dir = storage_dir.join(&fake_id);
                        fs_err::create_dir_all(&bucket_dir).unwrap();
                        fs_err::write(bucket_dir.join(MATH1_INPUT_FILENAME), MATH1_INPUT_JSON).unwrap();
                        pointer = format!(
                            r#"{{"type":"math1-input","bucket":"{fake_id}","filename":"{MATH1_INPUT_FILENAME}"}}"#
                        );
                    }
                    Ok(ServerMessage::ListAgentsResponse { agents }) => {
                        peers = agents
                            .into_iter()
                            .map(|summary| summary.agent_id)
                            .filter(|agent_id| *agent_id != fake_id)
                            .collect();
                    }
                    _ => {}
                },
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(err))) => return Err(err.into()),
                Ok(None) => return Err(Math1Error::Protocol("fake agent socket closed".to_string())),
                Err(_elapsed) => break,
            }
        }

        // The module writes its output into its own bucket; poll every known peer's bucket on disk.
        for peer in &peers {
            let output_path = storage_dir.join(peer).join(MATH1_OUTPUT_FILENAME);
            if let Ok(bytes) = fs_err::read(&output_path) {
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let weight = value
                    .get("weight")
                    .and_then(serde_json::Value::as_f64)
                    .ok_or_else(|| Math1Error::Protocol("output missing `weight`".to_string()))?;
                let bias = value
                    .get("bias")
                    .and_then(serde_json::Value::as_f64)
                    .ok_or_else(|| Math1Error::Protocol("output missing `bias`".to_string()))?;
                return Ok((weight, bias));
            }
        }

        // Ask for the roster and re-broadcast the pointer; both are safe to repeat.
        let list = serde_json::to_string(&ClientMessage::ListAgents)?;
        socket.send(Message::Text(list)).await?;
        if !pointer.is_empty() {
            socket.send(Message::Text(pointer.clone())).await?;
        }
    }
}

/// Check a module's global model against the expected weights for the canonical input.
pub fn verify_math1_model(weight: f64, bias: f64) -> Result<(), Math1Error> {
    if (weight - MATH1_EXPECTED_WEIGHT).abs() > MATH1_TOLERANCE {
        return Err(Math1Error::Protocol(format!(
            "weight {weight} != expected {MATH1_EXPECTED_WEIGHT}"
        )));
    }
    if (bias - MATH1_EXPECTED_BIAS).abs() > MATH1_TOLERANCE {
        return Err(Math1Error::Protocol(format!(
            "bias {bias} != expected {MATH1_EXPECTED_BIAS}"
        )));
    }
    Ok(())
}
