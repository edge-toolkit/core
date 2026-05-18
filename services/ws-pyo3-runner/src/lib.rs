//! Generic edge-toolkit agent runtime that hosts a user-supplied Python
//! module via `PyO3`. This crate is the Python sibling of
//! `et-ws-wasi-runner`: one binary, swappable user code, et-ws-server is
//! the always-on hub on the wire.
//!
//! Everything that matters lives in Python -- Rust just handles the
//! WebSocket transport, the et-* registration handshake, and dispatch
//! into `init` / `on_connect` / `on_text_frame` / `on_binary_frame` /
//! `on_shutdown`. The user module owns its state via module-level
//! globals; the runner never marshals state across the FFI boundary.
//! See `python/echo.py` for the contract.

#![expect(
    clippy::single_call_fn,
    clippy::integer_division_remainder_used,
    clippy::result_large_err,
    reason = "register/drive/storage_worker are single-use; select! uses %; RunnerError carries tungstenite::Error"
)]

pub mod agent;
pub mod config;
pub mod error;
pub mod python;
