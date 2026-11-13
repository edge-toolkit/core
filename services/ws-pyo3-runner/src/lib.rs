#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![expect(
    clippy::single_call_fn,
    reason = "register/drive/storage_worker/python_worker single-use; RunnerError: tungstenite::Error"
)]

pub mod agent;
pub mod config;
#[cfg(feature = "coverage")]
pub mod coverage;
pub mod error;
pub mod hub_module;
pub mod python;
