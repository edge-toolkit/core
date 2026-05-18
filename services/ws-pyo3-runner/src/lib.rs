//! Generic edge-toolkit agent runtime that hosts a user-supplied Python
//! module via PyO3. This crate is the Python sibling of
//! `et-ws-wasi-runner`: one binary, swappable user code, et-ws-server is
//! the always-on hub on the wire.
//!
//! Everything that matters lives in Python — Rust just handles the
//! WebSocket transport, the et-* registration handshake, and dispatch
//! into `init` / `on_connect` / `on_text_frame` / `on_binary_frame` /
//! `on_shutdown`. The user module owns its state via module-level
//! globals; the runner never marshals state across the FFI boundary.
//! See `python/echo.py` for the contract.

pub mod agent;
pub mod python;
