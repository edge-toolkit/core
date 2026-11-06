//! A board's store data, and its hosting of the `runner` world and of a core module's `et` imports: the guest's log
//! through the board, and its websocket and storage buckets the hub's, over the board's transport.

use alloc::collections::VecDeque;
#[cfg(not(feature = "std"))]
use alloc::{
    string::{String, ToString as _},
    vec::Vec,
};

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use wasmtime::component::{Resource, ResourceTable};

use super::memory::{Allowance, MemoryBudget};
use super::platform::{Platform, Transport};
use crate::bindings::et::ws_messages::messages::{
    ClientMessage as WitClientMessage, ServerMessage as WitServerMessage,
};
use crate::bindings::et::ws_wasi::ws::{State, WsError};
use crate::bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};
use crate::bindings::wasi::logging::logging::Level;
use crate::bindings::{et, wasi};
#[cfg(feature = "core-modules")]
use crate::convert::frame_of;
use crate::convert::{Frame, client_frame, server_message, server_message_to_wit};
#[cfg(feature = "core-modules")]
use crate::core_module::CoreHost;
use crate::keyvalue::{Bucket, bucket_from_identifier, kv_not_implemented};

/// How long `connect` waits for each frame while it waits for the hub's `et-connect-ack`.
const CONNECT_ACK_TIMEOUT_MS: u32 = 10_000;

/// The store data a board runs a guest with.
pub struct BoardHost<P, T> {
    /// The board's clock, log and console.
    platform: P,
    /// The guest's resources: its buckets, streams and pollables.
    table: ResourceTable,
    /// The hub: its websocket, and the storage the guest's storage buckets are in.
    transport: T,
    /// The guest's draw on the board's memory, which wasmtime asks before each memory or table grows.
    allowance: Allowance,
    /// Messages that arrived before the guest asked for them: the connect ack, and any that came ahead of it.
    inbox: VecDeque<ServerMessage>,
    /// The id the hub assigned, empty until it has.
    agent_id: String,
    /// The websocket's state, as the guest sees it.
    state: State,
}

impl<P, T> BoardHost<P, T> {
    /// A guest's store data, with no websocket open.
    ///
    /// The guest reaches the hub through `transport`, and keeps its memories in `memory`.
    #[cfg_attr(
        not(any(test, feature = "core-modules")),
        expect(
            clippy::single_call_fn,
            reason = "the store data's constructor, called where run makes the store"
        )
    )]
    pub(super) fn new(platform: P, transport: T, memory: MemoryBudget) -> Self {
        Self {
            platform,
            table: ResourceTable::new(),
            transport,
            allowance: Allowance::new(memory),
            inbox: VecDeque::new(),
            agent_id: String::default(),
            state: State::Closed,
        }
    }

    /// The guest's draw on the board's memory, which the store's limiter is.
    pub(super) const fn allowance(&mut self) -> &mut Allowance {
        &mut self.allowance
    }

    /// The board's clock, log and console, which the WASI functions use.
    pub(super) const fn platform(&self) -> &P {
        &self.platform
    }

    /// The guest's resources, which the WASI functions add their streams and pollables to.
    pub(super) const fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    /// Whether the websocket is open, as every call but `connect` needs it.
    const fn connected(&self) -> Result<(), WsError> {
        if matches!(self.state, State::Connected) {
            Ok(())
        } else {
            Err(WsError::NotConnected)
        }
    }
}

impl<P, T: Transport> BoardHost<P, T> {
    /// Send `frame` over the websocket, which the caller has checked is open.
    async fn send_frame(&mut self, frame: Frame) -> Result<(), WsError> {
        match self.transport.send(frame).await {
            Ok(()) => Ok(()),
            Err(reason) => Err(WsError::Transport(reason.to_string())),
        }
    }

    /// The next message from the hub: one that arrived while connecting, or the next frame within `timeout_ms`.
    async fn next_message(&mut self, timeout_ms: u32) -> Result<Option<ServerMessage>, WsError> {
        self.connected()?;
        if let Some(message) = self.inbox.pop_front() {
            return Ok(Some(message));
        }
        match self.transport.recv(timeout_ms).await {
            Ok(Some(frame)) => Ok(Some(server_message(frame)?)),
            Ok(None) => Ok(None),
            Err(reason) => Err(WsError::Transport(reason.to_string())),
        }
    }

    /// Send the hello over the open transport and wait for the hub's ack, queueing it and anything ahead of it.
    async fn register(&mut self) -> Result<(), WsError> {
        let hello = serde_json::to_string(&ClientMessage::Connect { agent_id: None })?;
        if let Err(reason) = self.transport.send(Frame::Text(hello)).await {
            return Err(WsError::Transport(reason.to_string()));
        }
        loop {
            let frame = match self.transport.recv(CONNECT_ACK_TIMEOUT_MS).await {
                Ok(Some(frame)) => frame,
                Ok(None) => return Err(WsError::Transport("the hub sent no et-connect-ack".to_string())),
                Err(reason) => return Err(WsError::Transport(reason.to_string())),
            };
            let message = server_message(frame)?;
            if let ServerMessage::ConnectAck { agent_id, .. } = &message {
                self.agent_id.clone_from(agent_id);
                self.inbox.push_front(message);
                self.state = State::Connected;
                return Ok(());
            }
            self.inbox.push_back(message);
        }
    }
}

/// What `bucket` holds under `key` on the hub `transport` reaches.
///
/// A module's bucket holds nothing, since a board serves no module's files.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "shared with a core module's kv_get when the crate hosts one"
    )
)]
async fn load<T>(transport: &mut T, bucket: &Bucket, key: &str) -> Result<Option<Vec<u8>>, StoreError>
where
    T: Transport,
{
    let Bucket::Storage { agent_id } = bucket else {
        return Ok(None);
    };
    match transport.load(agent_id, key).await {
        Ok(value) => Ok(value),
        Err(error) => Err(StoreError::Other(error.to_string())),
    }
}

/// Store `value` under `key` in `bucket` on the hub `transport` reaches, which refuses a module's bucket.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "shared with a core module's kv_set when the crate hosts one"
    )
)]
async fn store<T>(transport: &mut T, bucket: &Bucket, key: &str, value: Vec<u8>) -> Result<(), StoreError>
where
    T: Transport,
{
    let Bucket::Storage { agent_id } = bucket else {
        return Err(StoreError::AccessDenied);
    };
    match transport.store(agent_id, key, value).await {
        Ok(()) => Ok(()),
        Err(error) => Err(StoreError::Other(error.to_string())),
    }
}

impl<P: Platform, T: Transport> wasi::logging::logging::Host for BoardHost<P, T> {
    async fn log(&mut self, level: Level, context: String, message: String) {
        self.platform.log(level, &context, &message);
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the generated Host trait declares open async, and the buckets are in memory"
)]
impl<P: Platform, T: Transport> wasi::keyvalue::store::Host for BoardHost<P, T> {
    async fn open(&mut self, identifier: String) -> Result<Resource<Bucket>, StoreError> {
        let bucket = bucket_from_identifier(&identifier)?;
        Ok(self.table.push(bucket)?)
    }
}

impl<P: Platform, T: Transport> wasi::keyvalue::store::HostBucket for BoardHost<P, T> {
    async fn get(&mut self, self_: Resource<Bucket>, key: String) -> Result<Option<Vec<u8>>, StoreError> {
        load(&mut self.transport, self.table.get(&self_)?, &key).await
    }

    async fn set(&mut self, self_: Resource<Bucket>, key: String, value: Vec<u8>) -> Result<(), StoreError> {
        store(&mut self.transport, self.table.get(&self_)?, &key, value).await
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated HostBucket trait declares every method async, and a board refuses this one at once"
    )]
    async fn delete(&mut self, _bucket: Resource<Bucket>, _key: String) -> Result<(), StoreError> {
        Err(kv_not_implemented("delete"))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated HostBucket trait declares every method async, and a board refuses this one at once"
    )]
    async fn exists(&mut self, _bucket: Resource<Bucket>, _key: String) -> Result<bool, StoreError> {
        Err(kv_not_implemented("exists"))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated HostBucket trait declares every method async, and a board refuses this one at once"
    )]
    async fn list_keys(
        &mut self,
        _bucket: Resource<Bucket>,
        _cursor: Option<String>,
    ) -> Result<KeyResponse, StoreError> {
        Err(kv_not_implemented("list-keys"))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated HostBucket trait declares every method async, and a bucket handle is in memory"
    )]
    async fn drop(&mut self, rep: Resource<Bucket>) -> wasmtime::Result<()> {
        let _dropped: Bucket = self.table.delete(rep)?;
        Ok(())
    }
}

// The `et:ws-messages/messages` interface only declares types, but the linker still anchors it on a `Host` impl.
impl<P: Platform, T: Transport> et::ws_messages::messages::Host for BoardHost<P, T> {}

impl<P: Platform, T: Transport> et::ws_wasi::ws::Host for BoardHost<P, T> {
    /// Open the websocket and register, keeping the hub's ack for the guest's first `recv`, as the host runner does.
    async fn connect(&mut self) -> Result<(), WsError> {
        if matches!(self.state, State::Connected) {
            return Err(WsError::AlreadyConnected);
        }
        if let Err(reason) = self.transport.connect().await {
            return Err(WsError::Transport(reason.to_string()));
        }
        self.state = State::Connecting;
        let registered = self.register().await;
        if registered.is_err() {
            et::ws_wasi::ws::Host::disconnect(self).await;
        }
        registered
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated Host trait declares get-state async, and the state is a field"
    )]
    async fn get_state(&mut self) -> State {
        self.state
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated Host trait declares agent-id async, and the id is a field"
    )]
    async fn agent_id(&mut self) -> String {
        self.agent_id.clone()
    }

    async fn send(&mut self, message: WitClientMessage) -> Result<(), WsError> {
        self.connected()?;
        self.send_frame(client_frame(message)?).await
    }

    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<WitServerMessage>, WsError> {
        self.next_message(timeout_ms)
            .await?
            .map(server_message_to_wit)
            .transpose()
    }

    async fn disconnect(&mut self) {
        if !matches!(self.state, State::Closed) {
            self.transport.close().await;
        }
        self.state = State::Closed;
        self.inbox.clear();
        self.agent_id.clear();
    }
}

#[cfg(feature = "core-modules")]
impl<P: Platform, T: Transport> CoreHost for BoardHost<P, T> {
    /// Open the websocket and register, as a component's `connect` does.
    async fn connect(&mut self) -> Result<(), WsError> {
        et::ws_wasi::ws::Host::connect(self).await
    }

    /// The agent id, as a component's `agent-id` gives it.
    async fn agent_id(&mut self) -> String {
        et::ws_wasi::ws::Host::agent_id(self).await
    }

    /// Send `message` as the frame it goes out as.
    async fn send(&mut self, message: ClientMessage) -> Result<(), WsError> {
        self.connected()?;
        self.send_frame(frame_of(message)?).await
    }

    /// The next message, from the same inbox a component's `recv` drains.
    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<ServerMessage>, WsError> {
        self.next_message(timeout_ms).await
    }

    /// Close the websocket and forget what arrived on it, as a component's `disconnect` does.
    async fn disconnect(&mut self) {
        et::ws_wasi::ws::Host::disconnect(self).await;
    }

    /// What the hub stores under `key` in the bucket `bucket` names, reached over the board's transport.
    async fn get(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        load(&mut self.transport, &bucket_from_identifier(bucket)?, key).await
    }

    /// Have the hub store `value` under `key` in the bucket `bucket` names, over the board's transport.
    async fn set(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        store(&mut self.transport, &bucket_from_identifier(bucket)?, key, value).await
    }

    /// Record the entry through the board, as a component's `wasi:logging` call is.
    async fn log(&mut self, level: Level, context: &str, message: &str) {
        self.platform.log(level, context, message);
    }
}
