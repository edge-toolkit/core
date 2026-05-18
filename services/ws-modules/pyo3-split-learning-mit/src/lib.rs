//! Edge-toolkit agent that hosts the MIT split-learning-demo server-side
//! model in-process. PyTorch has no WASI/WASM build, so this crate is a
//! native binary (not a browser ws-module) that embeds CPython via PyO3 and
//! connects to et-ws-server as an agent.
//!
//! See the binary entry point in `bin/main.rs` and the agent loop in
//! `agent.rs`. The wire-format adapter (`wire.rs`) and PyO3 driver
//! (`python.rs`) are separated so the envelope codec can be unit-tested
//! without bringing PyTorch up.

pub mod agent;
pub mod python;
pub mod wire;
