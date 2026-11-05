#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(any(
    feature = "board",
    feature = "core-modules",
    feature = "precompile",
    not(feature = "std")
))]
extern crate alloc;

pub mod bindings;
#[cfg(feature = "board")]
pub mod board;
#[cfg(feature = "std")]
pub mod config;
pub mod convert;
#[cfg(feature = "core-modules")]
pub mod core_module;
#[cfg(any(feature = "board", feature = "precompile"))]
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
