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

/// One unit of work for the Python dispatch thread.
///
/// The WS loop forwards inbound frames plus the connect/shutdown lifecycle as
/// these; the worker drains them in submission order, off the WS task, so a
/// slow handler never stalls the heartbeat or the outbound drain.
#[derive(Debug)]
enum InboundEvent {
    Connect(String),
    Text(String),
    Binary(Vec<u8>),
    Shutdown,
}

/// Connection inputs the binary entrypoint hands to [`initialize`].
#[expect(
    clippy::exhaustive_structs,
    reason = "input config built by the binary entrypoint via a struct literal; new fields are additive there"
)]
pub struct AgentConfig {
    pub ws_url: String,
    /// Optional `agent_id` to request on connect.
    ///
    /// `None` lets the server assign a fresh one.
    pub requested_agent_id: Option<String>,
    /// How long to wait for `et-connect-ack`; `None` waits forever.
    pub connect_ack_timeout: Option<Duration>,
}

/// A built-but-not-yet-connected agent: channels, dispatcher, and config.
///
/// Produced by [`initialize`] and consumed by [`run`], which connects and
/// drives it.
#[non_exhaustive]
pub struct InitializedAgent {
    pub config: AgentConfig,
    pub dispatcher: Dispatcher,
    /// Reply-by-return path for handler return values.
    ///
    /// The Python dispatch worker pushes a handler's returned `bytes` / `str`
    /// onto the same outbound queue Python's `WsSender` writes to.
    pub outbound_tx: mpsc::UnboundedSender<OutboundFrame>,
    pub outbound_rx: mpsc::UnboundedReceiver<OutboundFrame>,
    /// Shared cell `WsStorage.put()` reads to learn our `agent_id`.
    ///
    /// The runner populates it after `et-connect-ack`.
    pub agent_id_slot: AgentIdSlot,
    /// Receiver half of the storage op channel, drained by the worker in `run()`.
    pub storage_rx: mpsc::UnboundedReceiver<StorageOp>,
    /// Base URL for storage requests, e.g. `http://127.0.0.1:8080`.
    pub http_base: String,
}

/// Build the channels, `WsSender`, and `WsStorage`, then import the module.
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

/// Connect, register, and drive the agent until the connection closes.
///
/// Spawns the storage worker and the Python dispatch thread, completes the
/// `et-connect` handshake, then runs the WS loop. Returns once the socket
/// closes or `drive` errors.
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

    // Run Python on its own OS thread: every hook executes here, off the async
    // WS task, so even a long-running handler can't stall the heartbeat or the
    // outbound drain in `drive`. The worker owns the Dispatcher and processes
    // inbound events in submission order.
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<InboundEvent>();
    let worker = std::thread::Builder::new()
        .name("pyo3-dispatch".to_owned())
        .spawn(move || python_worker(dispatcher, inbound_rx, outbound_tx))?;

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
    // Populate the slot before `on_connect` so Python sees a valid
    // `storage.agent_id` from the first instant it can act.
    *agent_id_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(agent_id.clone());
    let _connect_sent = inbound_tx.send(InboundEvent::Connect(agent_id));

    let result = drive(&mut socket, &inbound_tx, &mut outbound_rx).await;

    // Queue `on_shutdown` (the worker drains any frames ahead of it first),
    // then drop our sender so the worker's recv loop ends. Join before aborting
    // the storage task so an `on_shutdown` that persists state can still reach
    // it; only then close the socket and stop storage.
    let _shutdown_sent = inbound_tx.send(InboundEvent::Shutdown);
    drop(inbound_tx);
    let _joined = worker.join();
    let _close_sent = socket.send(tungstenite::Message::Close(None)).await;
    storage_task.abort();
    result
}

/// Run every Python hook on a dedicated OS thread, owning the `Dispatcher`.
///
/// Fully decouples Python execution from the async WS task. Handler return
/// values are pushed onto the same outbound queue Python's `WsSender` writes
/// to. Runs until the inbound channel closes (after `Shutdown`).
#[expect(
    clippy::cognitive_complexity,
    clippy::needless_pass_by_value,
    reason = "owns its args for the thread's lifetime; one linear match over the inbound event taxonomy"
)]
fn python_worker(
    dispatcher: Dispatcher,
    mut inbound_rx: mpsc::UnboundedReceiver<InboundEvent>,
    outbound_tx: mpsc::UnboundedSender<OutboundFrame>,
) {
    while let Some(event) = inbound_rx.blocking_recv() {
        match event {
            InboundEvent::Connect(agent_id) => {
                if let Err(err) = dispatcher.on_connect(&agent_id) {
                    warn!("on_connect hook failed: {err}");
                }
            }
            InboundEvent::Text(text) => match dispatcher.on_text_frame(&text) {
                Ok(Some(reply)) => {
                    let _sent = outbound_tx.send(OutboundFrame::Text(reply));
                }
                Ok(None) => {}
                Err(err) => warn!("on_text_frame raised: {err}"),
            },
            InboundEvent::Binary(bytes) => match dispatcher.on_binary_frame(&bytes) {
                Ok(Some(reply)) => {
                    let _sent = outbound_tx.send(OutboundFrame::Binary(reply));
                }
                Ok(None) => {}
                Err(err) => warn!("on_binary_frame raised: {err}"),
            },
            InboundEvent::Shutdown => {
                if let Err(err) = dispatcher.on_shutdown() {
                    warn!("on_shutdown hook failed: {err}");
                }
                break;
            }
        }
    }
}

/// Drain `StorageOp`s from the channel, resolving each via `et-rest-client`.
///
/// One worker handles all storage I/O for the agent -- the operations are
/// infrequent (load on connect, save on shutdown for the typical model-weights
/// case) so serial execution is fine. A missing key surfaces as the client's
/// `ErrorResponse` (the 404 arm), which we map to `Ok(None)`.
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
                let _reply_sent = reply.send(outcome);
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
                let _reply_sent = reply.send(outcome);
            }
        }
    }
}

/// Drive the socket in both directions.
///
/// Inbound frames are forwarded to the Python dispatch worker via `inbound_tx`
/// (a non-blocking send, so a slow handler never holds up this loop); outbound
/// frames the worker or Python's `WsSender` produced come back through
/// `outbound_rx` and out to the socket.
async fn drive(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    inbound_tx: &mpsc::UnboundedSender<InboundEvent>,
    outbound_rx: &mut mpsc::UnboundedReceiver<OutboundFrame>,
) -> Result<(), RunnerError> {
    // Keepalive: the server closes idle connections and never pings us, so a
    // module that only waits for inbound frames would be timed out. Ping on a
    // cadence well inside the server's timeout to stay registered.
    let mut heartbeat = et_ws_runner_common::heartbeat_interval().await;
    loop {
        tokio::select! {
            // Inbound: hand the frame to the dispatch worker and keep looping.
            // The worker emits any reply onto the same outbound queue Python
            // pushes to via WsSender, so multi-send + reply compose in order.
            frame = socket.next() => match frame {
                Some(Ok(tungstenite::Message::Binary(bytes))) => {
                    let _forwarded = inbound_tx.send(InboundEvent::Binary(bytes));
                }
                Some(Ok(tungstenite::Message::Text(text))) => {
                    let _forwarded = inbound_tx.send(InboundEvent::Text(text));
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
            // Outbound: drain anything the worker pushed (Python's WsSender
            // sends or return-value replies).
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
