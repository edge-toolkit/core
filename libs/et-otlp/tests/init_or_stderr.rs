//! Covers both answers `init_or_stderr` can give, which no single process could otherwise observe.
//!
//! Each arm installs process-global state -- a plain subscriber on one side, the whole `OTel` pipeline on the
//! other -- so a second call in the same process fails whichever arm it takes. The two live in separate tests
//! for that reason and no other: the runner harness gives each test its own process, which is what makes the
//! pair observable at all.
//!
//! What they protect is the decision, not the pipeline. A runner whose deployment configured `OTLP_*` and
//! silently got stderr logging looks healthy and reports nothing, which is how two of the three runners went
//! without telemetry unnoticed.
#![cfg(test)]

use edge_toolkit::config::OtlpConfig;
use et_otlp::init_or_stderr;

/// A collector that is not listening, which the exporters accept because they connect lazily.
///
/// Deserialised rather than built as a literal, both because the type is `#[non_exhaustive]` and because
/// that is how a runner obtains one: every field left out is a default a deployment would also inherit.
const UNREACHABLE_COLLECTOR: &str = r#"{"collector_url": "http://127.0.0.1:1/api/default/v1"}"#;

#[test]
fn no_config_falls_back_to_stderr_and_hands_back_no_handles() {
    // The absence of handles is the observable: it is what tells a caller there is nothing to flush.
    let handles = init_or_stderr(None).unwrap();

    assert!(handles.is_none());
}

#[test]
fn a_config_builds_the_pipeline_and_hands_back_handles_to_flush() {
    // Nothing here exports: the test asserts a pipeline was built and torn down, and reaching a collector is
    // the business of the end-to-end tests that run a real one.
    let config: OtlpConfig = serde_json::from_str(UNREACHABLE_COLLECTOR).unwrap();

    let handles = init_or_stderr(Some(&config)).unwrap();

    let Some(handles) = handles else {
        panic!("a configured runner must get handles back, or its spans are never flushed");
    };
    // Shutting down is the other half of the contract, and it is what every runner's exit path calls.
    handles.shutdown();
}
