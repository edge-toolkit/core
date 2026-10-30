#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod args;
pub mod auth;
#[cfg(feature = "std")]
pub mod config;
pub mod ports;
pub mod ws;
pub mod ws_server;
