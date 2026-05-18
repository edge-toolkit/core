//! WebSocket loop for the generic pyo3 runner.
//!
//! Same handshake as `et-ws-wasi-runner`: send `et-connect`, wait for
//! `et-connect-ack`, capture the assigned `agent_id`, and forward every
//! inbound frame to the user's Python module. Frames the module wants
//! to send go through a `WsSender` it received at `init()` time; the
//! WS loop drains the channel in parallel with the inbound stream via
//! `tokio::select!`, so Python can push frames whenever (during a
//! handler, after, or from a background thread).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use edge_toolkit::ws::{ConnectStatus, WsMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};
use tracing::{info, warn};

use crate::python::{Dispatcher, OutboundFrame, WsSender};

/// How long we'll wait for the server's `et-connect-ack` before giving up.
const CONNECT_ACK_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AgentConfig {
    pub ws_url: String,
    /// Optional `agent_id` to request on connect. `None` lets the server
    /// assign a fresh one.
    pub requested_agent_id: Option<String>,
}

pub struct InitializedAgent {
    pub config: AgentConfig,
    pub dispatcher: Dispatcher,
    /// Reply-by-return path. `drive()` clones this each handler call so a
    /// returned `bytes` / `str` lands on the same outbound queue Python's
    /// `WsSender` writes to.
    pub outbound_tx: mpsc::UnboundedSender<OutboundFrame>,
    pub outbound_rx: mpsc::UnboundedReceiver<OutboundFrame>,
}

/// Build the outbound channel + Sender, then import the Python module.
///
/// The Sender is built first so it can be handed to the module's
/// `init()` hook. The receiver stays with the caller — `run()` consumes
/// it inside the WS loop.
pub fn initialize(
    module_name: &str,
    python_path_extras: &[std::path::PathBuf],
    config: AgentConfig,
) -> Result<InitializedAgent> {
    let (tx, rx) = mpsc::unbounded_channel::<OutboundFrame>();
    let sender = WsSender::new(tx.clone());
    let dispatcher = Dispatcher::import(module_name, python_path_extras, sender)
        .map_err(|e| anyhow!("import python module `{module_name}`: {e}"))?;
    Ok(InitializedAgent {
        config,
        dispatcher,
        outbound_tx: tx,
        outbound_rx: rx,
    })
}

pub async fn run(agent: InitializedAgent) -> Result<()> {
    let InitializedAgent {
        config,
        dispatcher,
        outbound_tx,
        mut outbound_rx,
    } = agent;

    info!("connecting to {}", config.ws_url);
    let (mut socket, _) = connect_async(&config.ws_url)
        .await
        .with_context(|| format!("connect to {}", config.ws_url))?;

    let agent_id = register(&mut socket, config.requested_agent_id).await?;
    if let Err(err) = dispatcher.on_connect(&agent_id) {
        warn!("on_connect hook failed: {err}");
    }

    let result = drive(&mut socket, &dispatcher, &outbound_tx, &mut outbound_rx).await;
    if let Err(err) = dispatcher.on_shutdown() {
        warn!("on_shutdown hook failed: {err}");
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

/// Drive the socket in both directions. Inbound frames go to Python;
/// outbound frames Python pushed via `WsSender` come back through the
/// channel and out to the socket. Python errors are logged but don't
/// terminate the connection.
async fn drive(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    dispatcher: &Dispatcher,
    outbound_tx: &mpsc::UnboundedSender<OutboundFrame>,
    outbound_rx: &mut mpsc::UnboundedReceiver<OutboundFrame>,
) -> Result<()> {
    loop {
        tokio::select! {
            // Inbound: read a frame from the server and dispatch to Python.
            // Any reply the handler returns is appended to the same outbound
            // queue Python pushes to via WsSender, so multi-send + reply
            // compose in submission order.
            frame = socket.next() => match frame {
                Some(Ok(tungstenite::Message::Binary(bytes))) => {
                    match dispatcher.on_binary_frame(&bytes) {
                        Ok(Some(reply)) => {
                            let _ = outbound_tx.send(OutboundFrame::Binary(reply));
                        }
                        Ok(None) => {}
                        Err(err) => warn!("on_binary_frame raised: {err}"),
                    }
                }
                Some(Ok(tungstenite::Message::Text(text))) => {
                    match dispatcher.on_text_frame(&text) {
                        Ok(Some(reply)) => {
                            let _ = outbound_tx.send(OutboundFrame::Text(reply));
                        }
                        Ok(None) => {}
                        Err(err) => warn!("on_text_frame raised: {err}"),
                    }
                }
                Some(Ok(tungstenite::Message::Close(_))) => {
                    info!("server closed connection");
                    return Ok(());
                }
                Some(Ok(tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) | tungstenite::Message::Frame(_))) => {}
                Some(Err(e)) => return Err(e).context("ws recv"),
                None => return Ok(()),
            },
            // Outbound: drain anything Python pushed via WsSender or via
            // return-value replies enqueued above.
            Some(out) = outbound_rx.recv() => {
                let msg = match out {
                    OutboundFrame::Text(text) => tungstenite::Message::Text(text.into()),
                    OutboundFrame::Binary(bytes) => tungstenite::Message::Binary(bytes.into()),
                };
                socket.send(msg).await.context("send queued frame")?;
            }
        }
    }
}
