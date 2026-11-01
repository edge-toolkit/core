//! et-ws-server as firmware: the hub's `/ws`, `/health`, `/favicon.ico` and the modules built into it, on port 8080,
//! over a board's own Wi-Fi.
//!
//! The target picks the board: bare-metal Arm is the Raspberry Pi Pico 2 W, and bare-metal RISC-V the ESP32-C6. Built
//! for any target with an operating system, the same hub serves the same modules from the host, over tokio, so what a
//! board would serve can be explored without one.
#![cfg_attr(target_os = "none", no_std, no_main)]
#![recursion_limit = "256"]

#[cfg(target_os = "none")]
extern crate alloc;

use edge_toolkit::ports::Services;

#[cfg(target_os = "none")]
mod board;
#[cfg(not(target_os = "none"))]
mod host;

#[cfg(not(target_os = "none"))]
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
