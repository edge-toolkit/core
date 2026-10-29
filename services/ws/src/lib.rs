#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(any(not(feature = "std"), feature = "picoserve-hub"))]
extern crate alloc;

#[cfg(feature = "std")]
pub mod admin;
#[cfg(feature = "nats")]
mod backplane;
#[cfg(feature = "std")]
mod connection;
pub mod hub;
#[cfg(feature = "picoserve-hub")]
pub mod picoserve_hub;
#[cfg(feature = "std")]
mod server;
#[cfg(feature = "std")]
mod telemetry;

#[cfg(feature = "nats")]
pub use self::backplane::{
    Backplane, BackplaneConfig, BackplaneDelivery, BackplaneError, start_backplane, withdraw_local_agents,
};
pub use self::hub::SessionMessage;
#[cfg(feature = "std")]
pub use self::server::{
    AgentSession, DEFAULT_CONNECTION_TIMEOUT, DEFAULT_MAX_FRAME_SIZE, HEARTBEAT_INTERVAL, MessageRetention,
    WsAgentRegistry, WsConfig, configure, load_registry, ws_handler,
};
