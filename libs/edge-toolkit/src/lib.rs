#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

pub mod args;
pub mod auth;
pub mod config;
pub mod ports;
pub mod ws;
pub mod ws_server;
