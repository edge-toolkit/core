#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bindings;
pub mod board;
#[cfg(feature = "std")]
pub mod config;
pub mod convert;
pub mod engine;
#[cfg(feature = "std")]
pub mod error;
#[cfg(feature = "std")]
pub mod host;
pub mod keyvalue;

#[cfg(feature = "std")]
pub use self::error::RunnerError;
#[cfg(feature = "std")]
pub use self::host::{HostState, run_module};
