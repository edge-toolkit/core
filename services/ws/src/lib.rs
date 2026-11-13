#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(any(not(feature = "std"), feature = "board"))]
extern crate alloc;

#[cfg(feature = "std")]
pub mod admin;
#[cfg(feature = "nats")]
mod backplane;
#[cfg(feature = "std")]
mod connection;
pub mod health;
// The `no_std` routing core both transports share: the `std` server and a board's picoserve hub. It holds
// edge-toolkit's agent registry, which a build has with either; with neither, there is nothing for it to serve over.
#[cfg(any(feature = "board", feature = "std"))]
pub mod hub;
#[cfg(feature = "board")]
pub mod picoserve_hub;
#[cfg(feature = "std")]
mod server;
#[cfg(feature = "std")]
mod telemetry;

#[cfg(feature = "nats")]
pub use self::backplane::{
    Backplane, BackplaneConfig, BackplaneDelivery, BackplaneError, start_backplane, withdraw_local_agents,
};
#[cfg(any(feature = "board", feature = "std"))]
pub use self::hub::SessionMessage;
#[cfg(feature = "std")]
pub use self::server::{
    AgentSession, DEFAULT_CONNECTION_TIMEOUT, DEFAULT_MAX_FRAME_SIZE, HEARTBEAT_INTERVAL, MessageRetention,
    WsAgentRegistry, WsConfig, configure, load_registry, ws_handler,
};
