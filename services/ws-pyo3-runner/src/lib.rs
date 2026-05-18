//! Generic edge-toolkit agent runtime that hosts a user-supplied Python
//! module via PyO3. This crate is the Python sibling of
//! `et-ws-wasi-runner`: one binary, swappable user code, et-ws-server is
//! the always-on hub on the wire.
//!
//! Everything that matters lives in Python — Rust just handles the
//! WebSocket transport, the et-* registration handshake, and dispatch
//! into `init` / `set_agent_id` / `handle_text` / `handle_binary` /
//! `shutdown`. See `python/echo.py` for the contract.

pub mod agent;
pub mod python;
