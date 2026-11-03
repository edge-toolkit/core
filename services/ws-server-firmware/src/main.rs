//! The hub as firmware: its `/ws`, `/health`, `/favicon.ico`, agent storage and the modules built into it, on port
//! 8080, over a board's own Wi-Fi.
//!
//! The target picks the board: bare-metal Arm is the Raspberry Pi Pico 2 W, or the Pimoroni Pico Plus 2 W with its
//! feature, and bare-metal RISC-V the ESP32-C6. The Pico Plus 2 W also runs the wasi runners the build compiled in
//! beside the hub. Built for any target with an operating system, the same hub serves the same modules and runs the
//! same runners from the host, over tokio, so what a board would do can be explored without one.
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(target_os = "none", no_std, no_main)]
#![recursion_limit = "256"]

extern crate alloc;

#[cfg(not(remote_hub))]
use edge_toolkit::ports::Services;

#[cfg(target_os = "none")]
mod board;
#[cfg(not(target_os = "none"))]
mod host;
#[cfg(not(remote_hub))]
mod local;
mod runner;
#[cfg(not(remote_hub))]
mod server;

#[cfg(not(target_os = "none"))]
use self::host::main;

/// The port the hub serves on, as the `std` server's plain-HTTP listener does.
#[cfg(not(remote_hub))]
const PORT: u16 = Services::InsecureWebSocketServer.port();
/// The largest WebSocket frame the hub reads; each connection holds a buffer this size on the heap.
#[cfg(not(remote_hub))]
const MAX_FRAME_SIZE: usize = 8 * 1024;
/// How many messages for one connection the hub holds while it writes earlier ones, each up to `MAX_FRAME_SIZE`.
#[cfg(not(remote_hub))]
const MAX_QUEUED: usize = 2;
/// Each connection's buffer for the HTTP request head.
#[cfg(not(remote_hub))]
const HTTP_BUFFER: usize = 2048;

// The hub's modules, which a build for a hub elsewhere serves none of.
#[cfg(not(remote_hub))]
include!(concat!(env!("OUT_DIR"), "/modules.rs"));
// The hub elsewhere a build for one reaches.
#[cfg(remote_hub)]
include!(concat!(env!("OUT_DIR"), "/hub_address.rs"));
