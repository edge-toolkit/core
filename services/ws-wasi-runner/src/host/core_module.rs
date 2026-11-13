//! Run a core module on `SpaceWasm`, its `et` imports carried out on the same [`HostState`] a component's are.

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use tracing::{Instrument as _, Span};

use super::wasi_keyvalue::{bucket_get, bucket_set};
use super::ws::{next_message, send_frame};
use crate::bindings::et::ws_wasi::ws::{self, WsError};
use crate::bindings::wasi::keyvalue::store::Error as StoreError;
use crate::bindings::wasi::logging::logging::{self, Level};
use crate::convert::frame_of;
use crate::core_module::{self, CoreHost};
use crate::keyvalue::bucket_from_identifier;
use crate::{HostState, RunnerError};

impl CoreHost for HostState {
    /// Open the websocket as a component's `connect` does.
    async fn connect(&mut self) -> Result<(), WsError> {
        ws::Host::connect(self).await
    }

    /// The agent id, as a component's `agent-id` gives it.
    async fn agent_id(&mut self) -> String {
        ws::Host::agent_id(self).await
    }

    /// Send `message` as the frame it goes out as.
    async fn send(&mut self, message: ClientMessage) -> Result<(), WsError> {
        send_frame(&self.ws, frame_of(message)?).await
    }

    /// The next message, from the same inbox a component's `recv` drains.
    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<ServerMessage>, WsError> {
        next_message(&self.ws, timeout_ms).await
    }

    /// Close the websocket as a component's `disconnect` does.
    async fn disconnect(&mut self) {
        ws::Host::disconnect(self).await;
    }

    /// The value under `key` in the bucket `bucket` names, read as a component's `get` reads it.
    async fn get(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        bucket_get(&self.rest, &bucket_from_identifier(bucket)?, key).await
    }

    /// Store `value` under `key` in the bucket `bucket` names, as a component's `set` does.
    async fn set(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        bucket_set(&self.rest, &bucket_from_identifier(bucket)?, key, value).await
    }

    /// Record the entry as a component's `wasi:logging` call is.
    async fn log(&mut self, level: Level, context: &str, message: &str) {
        logging::Host::log(self, level, context.to_owned(), message.to_owned()).await;
    }
}

/// Run the core module `wasm` on `SpaceWasm` until its `run` returns, its imports carried out on `host`.
///
/// `SpaceWasm`'s engine cannot move between threads, so the module runs on a thread of its own, driven by the current
/// runtime's handle. That keeps this future `Send`, keeps a guest that computes for a long time off the runtime's
/// workers, and leaves nothing for the runtime to wait on at shutdown when the runner gives up on the guest.
///
/// Dropping this future stops the module, as dropping a component's run does: the thread stops at the guest's next
/// slice of instructions, or in the middle of a host call, and drops `host` without calling it again.
///
/// # Errors
///
/// When the thread cannot be started, the module fails as [`core_module::run`] describes, or the thread dies first.
pub async fn run_core_module<H>(wasm: Vec<u8>, host: H) -> Result<(), RunnerError>
where
    H: CoreHost + Send + 'static,
{
    let runtime = tokio::runtime::Handle::current();
    let span = Span::current();
    let (mut sender, receiver) = tokio::sync::oneshot::channel();
    let _thread = std::thread::Builder::new()
        .name("spacewasm".to_owned())
        .spawn(move || {
            runtime.block_on(async move {
                tokio::select! {
                    ended = core_module::run(&wasm, host).instrument(span) => {
                        // The receiver can go between the run ending and this send, leaving nobody to tell.
                        let _unheard: Result<(), _> = sender.send(ended);
                    }
                    // The runner stopped waiting, so the run is dropped where it stands, and `host` with it.
                    () = sender.closed() => {}
                }
            });
        })?;
    Ok(receiver.await??)
}
