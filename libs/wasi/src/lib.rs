//! Shared helpers for WIT-binding host impls and WASI guest modules.
//!
//! Right now this is just the [`error`] module — a tiny trait that
//! maps any `Display` error to the `Result<_, String>` shape required at
//! the WIT boundary. Future helpers that need to be available to both
//! `et-ws-wasi-runner` and the WASI guest modules (`wasi-comm1`,
//! `wasi-data1`, …) should land here too.

#![no_std]

pub mod error;
