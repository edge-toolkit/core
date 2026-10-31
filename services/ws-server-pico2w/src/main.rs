//! et-ws-server as firmware for the Raspberry Pi Pico 2 W: the hub's `/ws`, `/health`, `/favicon.ico` and the modules
//! built into it, on port 8080, over the board's own Wi-Fi.
//!
//! Built for any other target, the same hub serves the same modules from the host, over tokio, so what the board would
//! serve can be explored without one.
#![cfg_attr(all(target_arch = "arm", target_os = "none"), no_std, no_main)]
#![recursion_limit = "256"]

#[cfg(all(target_arch = "arm", target_os = "none"))]
extern crate alloc;

use edge_toolkit::ports::Services;

#[cfg(all(target_arch = "arm", target_os = "none"))]
mod firmware;
#[cfg(not(all(target_arch = "arm", target_os = "none")))]
mod host;

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
use self::host::main;

/// The port the hub serves on, as the `std` server's plain-HTTP listener does.
const PORT: u16 = Services::InsecureWebSocketServer.port();
/// The largest WebSocket frame the hub reads; each connection holds a buffer this size on the heap.
const MAX_FRAME_SIZE: usize = 8 * 1024;
/// How many messages for one connection the hub holds while it writes earlier ones, each up to `MAX_FRAME_SIZE`.
const MAX_QUEUED: usize = 2;
/// Each connection's buffer for the HTTP request head.
const HTTP_BUFFER: usize = 2048;

include!(concat!(env!("OUT_DIR"), "/modules.rs"));
