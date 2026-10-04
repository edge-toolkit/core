//! Covers each runner's `runner_config!` config: what serde-env reads into it, and its `Telemetered` view.
#![cfg(test)]

use std::path::PathBuf;

use et_otlp::Telemetered as _;
use et_ws_runner_common::{pyo3_config, wasi_config, web_config};

#[test]
fn wasi_config_reads_the_shared_groups_and_reports_no_otlp_when_unset() {
    let config: wasi_config::Config = serde_env::from_iter([("RUNNER_MODULE", "et-ws-wasi-data1")]).unwrap();

    assert_eq!(config.runner.module, "et-ws-wasi-data1");
    assert!(config.otlp().is_none());
}

#[test]
fn web_config_reads_v8_flags_and_reports_its_otlp_group() {
    let config: web_config::Config = serde_env::from_iter([
        ("RUNNER_MODULE", "et-ws-data1"),
        ("V8_FLAGS", "--no-liftoff"),
        ("OTLP_COLLECTOR_URL", "http://host:4318"),
    ])
    .unwrap();

    assert_eq!(config.v8_flags.as_deref(), Some("--no-liftoff"));
    assert_eq!(config.otlp().unwrap().collector_url, "http://host:4318");
}

#[test]
fn pyo3_config_splits_its_python_path_dropping_empty_segments() {
    let config: pyo3_config::Config = serde_env::from_iter([
        ("RUNNER_MODULE", "echo"),
        ("PYO3_PYTHONPATH", "/a::/b"),
        ("PYO3_AGENT_ID", "agent-7"),
    ])
    .unwrap();

    assert_eq!(config.pyo3.python_path(), [PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(config.pyo3.agent_id.as_deref(), Some("agent-7"));
    assert!(config.otlp().is_none());
}
