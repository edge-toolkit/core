//! WebSocket loop for the generic pyo3 runner.
//!
//! Same handshake as `et-ws-wasi-runner`: send `et-connect`, wait for
//! `et-connect-ack`, capture the assigned `agent_id`, and forward every
//! inbound frame to the user's Python module. The runner deliberately
//! makes no assumptions about the wire format — text and binary frames are
//! passed through verbatim — so all encoding lives Python-side.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use edge_toolkit::ws::{ConnectStatus, WsMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};
use tracing::{info, warn};

use crate::python::Dispatcher;

/// How long we'll wait for the server's `et-connect-ack` before giving up.
const CONNECT_ACK_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AgentConfig {
    pub ws_url: String,
    /// Optional `agent_id` to request on connect. `None` lets the server
    /// assign a fresh one.
    pub requested_agent_id: Option<String>,
    pub dispatcher: Dispatcher,
}

pub async fn run(config: AgentConfig) -> Result<()> {
    let AgentConfig {
        ws_url,
        requested_agent_id,
        dispatcher,
    } = config;

    info!("connecting to {ws_url}");
    let (mut socket, _) = connect_async(&ws_url)
        .await
        .with_context(|| format!("connect to {ws_url}"))?;

    let agent_id = register(&mut socket, requested_agent_id).await?;
    if let Err(err) = dispatcher.set_agent_id(&agent_id) {
        warn!("set_agent_id hook failed: {err}");
    }

    let result = drive(&mut socket, &dispatcher).await;
    if let Err(err) = dispatcher.shutdown() {
        warn!("shutdown hook failed: {err}");
    }
    let _ = socket.send(tungstenite::Message::Close(None)).await;
    result
}

async fn register(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    requested_agent_id: Option<String>,
) -> Result<String> {
    let connect = serde_json::to_string(&WsMessage::Connect {
        agent_id: requested_agent_id,
    })
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
    Ok(agent_id)
}

/// Inbound loop. Each frame is forwarded into Python; whatever the module
/// returns (if anything) is sent back as the same kind of frame. Errors
/// from Python don't terminate the connection — we log and continue.
async fn drive(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>, dispatcher: &Dispatcher) -> Result<()> {
    while let Some(frame) = socket.next().await {
        let frame = frame.context("ws recv")?;
        match frame {
            tungstenite::Message::Binary(bytes) => match dispatcher.handle_binary(&bytes) {
                Ok(Some(reply)) => {
                    socket
                        .send(tungstenite::Message::Binary(reply.into()))
                        .await
                        .context("send binary reply")?;
                }
                Ok(None) => {}
                Err(err) => warn!("handle_binary raised: {err}"),
            },
            tungstenite::Message::Text(text) => match dispatcher.handle_text(&text) {
                Ok(Some(reply)) => {
                    socket
                        .send(tungstenite::Message::Text(reply.into()))
                        .await
                        .context("send text reply")?;
                }
                Ok(None) => {}
                Err(err) => warn!("handle_text raised: {err}"),
            },
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
