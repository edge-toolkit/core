//! et-ws-server as firmware for the Raspberry Pi Pico 2 W: the hub's `/ws`, `/health` and `/favicon.ico` on port
//! 8080, over the board's own Wi-Fi.
#![cfg_attr(all(target_arch = "arm", target_os = "none"), no_std, no_main)]
#![recursion_limit = "256"]

#[cfg(all(target_arch = "arm", target_os = "none"))]
extern crate alloc;

#[cfg(all(target_arch = "arm", target_os = "none"))]
mod firmware;

/// Anywhere but on the board there is nothing to run; the binary exists there so the workspace's host builds compile.
#[cfg(not(all(target_arch = "arm", target_os = "none")))]
fn main() {}
