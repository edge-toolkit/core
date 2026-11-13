#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod args;
pub mod auth;
pub mod board;
#[cfg(feature = "std")]
pub mod config;
pub mod ports;
pub mod ws;
// The agent registry needs a lock: `std`'s, or on a board a spin lock. A `no_std` build for neither has no registry.
#[cfg(any(feature = "board", feature = "std"))]
pub mod ws_server;
