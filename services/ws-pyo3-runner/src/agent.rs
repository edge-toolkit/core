//! WebSocket loop for the generic pyo3 runner.
//!
//! Same handshake as `et-ws-wasi-runner`: send `et-connect`, wait for
//! `et-connect-ack`, capture the assigned `agent_id`, and forward every
//! inbound frame to the user's Python module. Frames the module wants
//! to send go through a `WsSender` it received at `init()` time; the
//! WS loop drains the channel in parallel with the inbound stream via
//! `tokio::select!`, so Python can push frames whenever (during a
//! handler, after, or from a background thread).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};
use tracing::{info, warn};

use crate::error::RunnerError;
use crate::python::{AgentIdSlot, Dispatcher, OutboundFrame, StorageError, StorageOp, WsSender, WsStorage};

#[expect(
    clippy::exhaustive_structs,
    reason = "input config built by the binary entrypoint via a struct literal; new fields are additive there"
)]
pub struct AgentConfig {
    pub ws_url: String,
    /// Optional `agent_id` to request on connect. `None` lets the server
    /// assign a fresh one.
    pub requested_agent_id: Option<String>,
    /// How long to wait for `et-connect-ack`; `None` waits forever.
    pub connect_ack_timeout: Option<Duration>,
}

#[non_exhaustive]
pub struct InitializedAgent {
    pub config: AgentConfig,
    pub dispatcher: Dispatcher,
    /// Reply-by-return path. `drive()` clones this each handler call so a
    /// returned `bytes` / `str` lands on the same outbound queue Python's
    /// `WsSender` writes to.
    pub outbound_tx: mpsc::UnboundedSender<OutboundFrame>,
    pub outbound_rx: mpsc::UnboundedReceiver<OutboundFrame>,
    /// Shared cell `WsStorage.put()` reads to know our `agent_id`. The
    /// runner populates it after `et-connect-ack`.
    pub agent_id_slot: AgentIdSlot,
    /// Receiver half of the storage op channel -- drained by the
    /// dedicated worker task in `run()`.
    pub storage_rx: mpsc::UnboundedReceiver<StorageOp>,
    /// Base URL for storage requests, e.g. `http://127.0.0.1:8080`.
    pub http_base: String,
}

/// Build the outbound channel + Sender + Storage, then import the
/// Python module.
///
/// The Sender and Storage are built first so they can be handed to the
/// module's `init(send, storage)` hook. The Storage's `agent_id` is
/// initially `None`; the runner fills it in after the server replies
/// with `et-connect-ack`. Storage ops are dispatched through an mpsc
/// channel into a worker task that owns the typed REST client.
pub fn initialize(
    module_name: &str,
    python_path_extras: &[std::path::PathBuf],
    config: AgentConfig,
) -> Result<InitializedAgent, RunnerError> {
    let (tx, rx) = mpsc::unbounded_channel::<OutboundFrame>();
    let sender = WsSender::new(tx.clone());

    let http_base = et_ws_runner_common::derive_http_base(&config.ws_url)?;
    let agent_id_slot: AgentIdSlot = Arc::new(Mutex::new(None));
    let (storage_tx, storage_rx) = mpsc::unbounded_channel::<StorageOp>();
    let storage = WsStorage::new(Arc::clone(&agent_id_slot), storage_tx);

    // Prepend mise-managed pipx `site-packages` so the module can `import`
    // packages preinstalled via mise (e.g. cowsay) without the operator setting
    // PYTHONPATH by hand. The explicit PYO3_PYTHONPATH entries come last so they
    // keep priority -- `Dispatcher::import` inserts each at `sys.path[0]`, so the
    // last entry wins.
    let mut python_path = edge_toolkit::config::mise_python_site_packages();
    python_path.extend_from_slice(python_path_extras);
    let dispatcher = Dispatcher::import(module_name, &python_path, sender, storage)?;
    Ok(InitializedAgent {
        config,
        dispatcher,
        outbound_tx: tx,
        outbound_rx: rx,
        agent_id_slot,
        storage_rx,
        http_base,
    })
}

#[expect(
    clippy::cognitive_complexity,
    reason = "linear startup sequence (worker spawn, connect, drive, shutdown) reads as one unit"
)]
pub async fn run(agent: InitializedAgent) -> Result<(), RunnerError> {
    let InitializedAgent {
        config,
        dispatcher,
        outbound_tx,
        mut outbound_rx,
        agent_id_slot,
        storage_rx,
        http_base,
    } = agent;

    // Spawn the storage worker first so it's ready by the time Python's
    // `init(send, storage)` returns. The worker outlives `run()` until
    // the channel is dropped -- i.e. when WsStorage (held by Python) is
    // dropped at process exit.
    let storage_task = tokio::spawn(storage_worker(http_base, storage_rx));

    info!("connecting to {}", config.ws_url);
    let (mut socket, agent_id, status) = et_ws_runner_common::connect_and_register(
        &config.ws_url,
        config.requested_agent_id,
        config.connect_ack_timeout,
    )
    .await?;
    info!(
        "registered as agent_id={agent_id} ({})",
        et_ws_runner_common::connect_status_label(&status)
    );
    // Populate the slot before calling `on_connect` so Python sees a
    // valid `storage.agent_id` from the first instant it can act.
    *agent_id_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(agent_id.clone());
    if let Err(err) = dispatcher.on_connect(&agent_id) {
        warn!("on_connect hook failed: {err}");
    }

    let result = drive(&mut socket, &dispatcher, &outbound_tx, &mut outbound_rx).await;
    if let Err(err) = dispatcher.on_shutdown() {
        warn!("on_shutdown hook failed: {err}");
    }
    drop(socket.send(tungstenite::Message::Close(None)).await);
    storage_task.abort();
    result
}

/// Run forever, draining `StorageOp`s from the channel and resolving each
/// through the generated `et-rest-client`. One worker handles all storage
/// I/O for the agent -- the operations are infrequent (load on connect, save
/// on shutdown for the typical model-weights case) so serial execution is
/// fine. A missing key surfaces as the client's `ErrorResponse` (the 404
/// arm), which we map to `Ok(None)`.
async fn storage_worker(http_base: String, mut rx: mpsc::UnboundedReceiver<StorageOp>) {
    let client = et_rest_client::Client::new(&http_base);
    while let Some(op) = rx.recv().await {
        match op {
            StorageOp::Get { agent_id, key, reply } => {
                let outcome = match client.get_file(&agent_id, &key).await {
                    Ok(response) => match et_ws_runner_common::collect_byte_stream(response.into_inner()).await {
                        Ok(bytes) => Ok(Some(bytes)),
                        Err(source) => Err(StorageError::get(&agent_id, &key, format!("reading body: {source}"))),
                    },
                    Err(et_rest_client::Error::ErrorResponse(_)) => Ok(None),
                    Err(source) => Err(StorageError::get(&agent_id, &key, source.to_string())),
                };
                drop(reply.send(outcome));
            }
            StorageOp::Put {
                agent_id,
                key,
                data,
                reply,
            } => {
                let outcome = match client.put_file(&agent_id, &key, data).await {
                    Ok(_) => Ok(()),
                    Err(source) => Err(StorageError::put(&agent_id, &key, source.to_string())),
                };
                drop(reply.send(outcome));
            }
        }
    }
}

/// Drive the socket in both directions. Inbound frames go to Python;
/// outbound frames Python pushed via `WsSender` come back through the
/// channel and out to the socket. Python errors are logged but don't
/// terminate the connection.
#[expect(
    clippy::cognitive_complexity,
    reason = "one select! loop matching the small inbound frame taxonomy; splitting it would obscure the flow"
)]
async fn drive(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    dispatcher: &Dispatcher,
    outbound_tx: &mpsc::UnboundedSender<OutboundFrame>,
    outbound_rx: &mut mpsc::UnboundedReceiver<OutboundFrame>,
) -> Result<(), RunnerError> {
    // Keepalive: the server closes idle connections and never pings us, so a
    // module that only waits for inbound frames would be timed out. Ping on a
    // cadence well inside the server's timeout to stay registered.
    let mut heartbeat = et_ws_runner_common::heartbeat_interval().await;
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
                            drop(outbound_tx.send(OutboundFrame::Binary(reply)));
                        }
                        Ok(None) => {}
                        Err(err) => warn!("on_binary_frame raised: {err}"),
                    }
                }
                Some(Ok(tungstenite::Message::Text(text))) => {
                    match dispatcher.on_text_frame(&text) {
                        Ok(Some(reply)) => {
                            drop(outbound_tx.send(OutboundFrame::Text(reply)));
                        }
                        Ok(None) => {}
                        Err(err) => warn!("on_text_frame raised: {err}"),
                    }
                }
                Some(Ok(tungstenite::Message::Close(_))) => {
                    info!("server closed connection");
                    return Ok(());
                }
                // Ping / Pong / Frame and any future variant: nothing to do.
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(RunnerError::WebSocket(e)),
                None => return Ok(()),
            },
            // Outbound: drain anything Python pushed via WsSender or via
            // return-value replies enqueued above.
            Some(out) = outbound_rx.recv() => {
                let msg = match out {
                    OutboundFrame::Text(text) => tungstenite::Message::Text(text),
                    OutboundFrame::Binary(bytes) => tungstenite::Message::Binary(bytes),
                };
                socket.send(msg).await?;
            }
            // Keepalive ping; the server treats it as activity and pongs back.
            _ = heartbeat.tick() => {
                socket.send(tungstenite::Message::Ping(Vec::new())).await?;
            }
        }
    }
}
