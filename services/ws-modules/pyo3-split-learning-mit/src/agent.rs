//! WebSocket agent loop: connects to et-ws-server, registers as an et agent,
//! and forwards split-learning demo frames between the wire and the embedded
//! PyTorch model.
//!
//! Frames the demo client sends are **binary** WebSocket frames whose body is
//! `base64(utf8(json(WSMessage)))`. Under the aligned protocol those frames
//! are unrecognised by et-ws-server, so the server default-broadcasts them to
//! every other connected agent — including this one. We process and reply
//! by sending another binary frame; the server default-broadcasts that back.
//!
//! For the canonical demo deployment (one demo client + this agent) the
//! "broadcast to all peers" semantics collapse to a direct request/response
//! exchange, so the wire is interchangeable with the original `server.py`.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use edge_toolkit::ws::{ConnectStatus, WsMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};
use tracing::{info, warn};

use crate::python::ModelHandle;
use crate::wire::{InboundKind, decode_inbound, encode_grads, encode_logits};

/// How long we'll wait for the server's `et-connect-ack` before giving up.
const CONNECT_ACK_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AgentConfig {
    pub ws_url: String,
    pub model: ModelHandle,
}

pub async fn run(config: AgentConfig) -> Result<()> {
    let AgentConfig { ws_url, model } = config;
    info!("connecting to {ws_url}");
    let (mut socket, _) = connect_async(&ws_url)
        .await
        .with_context(|| format!("connect to {ws_url}"))?;

    register(&mut socket).await?;

    let result = drive(&mut socket, &model).await;
    // Always export weights on the way out — mirrors server.py's
    // WebSocketDisconnect handler.
    if let Err(err) = model.export_weights() {
        warn!("ONNX export failed: {err}");
    }
    // Best-effort close so the server logs a clean disconnect.
    let _ = socket.send(tungstenite::Message::Close(None)).await;
    result
}

async fn register(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Result<()> {
    let connect = serde_json::to_string(&WsMessage::Connect { agent_id: None })
        .map_err(|e| anyhow!("serialize et-connect: {e}"))?;
    socket
        .send(tungstenite::Message::Text(connect.into()))
        .await
        .context("send et-connect")?;

    let ack = tokio::time::timeout(CONNECT_ACK_TIMEOUT, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.context("ws recv")?;
            let tungstenite::Message::Text(text) = frame else {
                continue;
            };
            let parsed: WsMessage = serde_json::from_str(&text)
                .with_context(|| format!("parse server frame: {text}"))?;
            if let WsMessage::ConnectAck { agent_id, status } = parsed {
                return Ok::<_, anyhow::Error>((agent_id, status));
            }
        }
        Err(anyhow!("server closed before et-connect-ack"))
    })
    .await
    .context("timed out waiting for et-connect-ack")??;

    let (agent_id, status) = ack;
    let kind = match status {
        ConnectStatus::Assigned => "assigned",
        ConnectStatus::Reconnected => "reconnected",
    };
    info!("registered as agent_id={agent_id} ({kind})");
    Ok(())
}

/// Inner inbound loop. Each binary frame is decoded via `wire::decode_inbound`,
/// dispatched through PyO3, and the response encoded back as a binary frame.
///
/// Text frames are logged and ignored — they're either et-protocol envelopes
/// (et-message-status, et-agent-message, etc.) or other peers' broadcasts.
async fn drive(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>, model: &ModelHandle) -> Result<()> {
    while let Some(frame) = socket.next().await {
        let frame = frame.context("ws recv")?;
        match frame {
            tungstenite::Message::Binary(bytes) => {
                if let Err(err) = handle_binary(socket, model, &bytes).await {
                    warn!("dropping binary frame: {err:#}");
                }
            }
            tungstenite::Message::Text(text) => {
                tracing::debug!("ignoring server-side text frame: {text}");
            }
            tungstenite::Message::Close(_) => {
                info!("server closed connection");
                return Ok(());
            }
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {}
            tungstenite::Message::Frame(_) => {}
        }
    }
    Ok(())
}

async fn handle_binary(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    model: &ModelHandle,
    bytes: &[u8],
) -> Result<()> {
    let parsed = decode_inbound(bytes).context("decode inbound frame")?;
    match parsed.kind {
        InboundKind::ActivationsAndLabels => {
            let labels = parsed
                .labels_bytes
                .as_deref()
                .ok_or_else(|| anyhow!("activations_and_labels frame missing labels"))?;
            let (grad_bytes, grad_shape, loss) = model
                .process_activations_and_labels(&parsed.tensor_bytes, labels, &parsed.tensor_shape)
                .map_err(|e| anyhow!("training step: {e}"))?;
            let response = encode_grads(&grad_bytes, &grad_shape, loss).context("encode grads")?;
            socket
                .send(tungstenite::Message::Binary(response.into()))
                .await
                .context("send grads")?;
            tracing::info!("training step: loss={loss:.4}");
        }
        InboundKind::Activations => {
            let (logits, shape) = model
                .process_activations(&parsed.tensor_bytes, &parsed.tensor_shape)
                .map_err(|e| anyhow!("inference step: {e}"))?;
            let response = encode_logits(&logits, &shape).context("encode logits")?;
            socket
                .send(tungstenite::Message::Binary(response.into()))
                .await
                .context("send logits")?;
            tracing::info!("inference step: logits_shape={shape:?}");
        }
    }
    Ok(())
}
