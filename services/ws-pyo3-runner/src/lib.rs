#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "register/drive/storage_worker/python_worker single-use; select! uses %; RunnerError: tungstenite::Error"
)]

pub mod agent;
pub mod config;
pub mod error;
pub mod hub_module;
pub mod python;
