//! Covers `is_module_file_missing`, which decides whether a failed fetch was an answer or a fault.
//!
//! A caller asking the hub for an optional asset treats "no such file" as a result and everything else as an error,
//! so the three outcomes are pinned here: the hub answering 404, the hub answering something else, and a failure that
//! never reached a hub at all. Getting the last two wrong turns a broken deployment into a silent empty value, which
//! surfaces far from the cause.
#![cfg(test)]

use et_test_helpers::{http_answering, reserve_port};
use et_ws_runner_common::{BootstrapError, fetch_module_file, is_module_file_missing};

/// The error from asking a hub for a file, with the hub answering `status`.
async fn fetch_failure(status: &'static str) -> BootstrapError {
    let (port, _requests) = http_answering(status);

    fetch_module_file(
        &et_rest_client::Client::new(&format!("http://127.0.0.1:{port}")),
        "@edge-toolkit/et-ws-math1",
        "mnist-12.onnx",
    )
    .await
    .unwrap_err()
}

#[tokio::test]
async fn a_404_from_the_hub_is_the_file_being_absent() {
    assert!(is_module_file_missing(&fetch_failure("404 Not Found").await));
}

#[tokio::test]
async fn another_status_from_the_hub_stays_an_error() {
    // A hub that answered at all but refused is a fault, not an absence: treating it as "no such file" would hand the
    // caller an empty value for a module the hub does serve.
    assert!(!is_module_file_missing(
        &fetch_failure("500 Internal Server Error").await
    ));
}

#[tokio::test]
async fn a_hub_that_was_never_reached_stays_an_error() {
    // Nothing is listening, so the failure carries no status at all -- the request never got far enough to be answered.
    // That is the case where reading absence into the error would hide an unreachable hub.
    let port = reserve_port();

    let error = fetch_module_file(
        &et_rest_client::Client::new(&format!("http://127.0.0.1:{port}")),
        "@edge-toolkit/et-ws-math1",
        "package.json",
    )
    .await
    .unwrap_err();

    assert!(matches!(&error, BootstrapError::Stream(_)), "got: {error:?}");
    assert!(!is_module_file_missing(&error));
}

#[test]
fn a_failure_that_is_not_a_request_at_all_stays_an_error() {
    // The remaining arm: a module whose `package.json` parsed but named no entry point never involved a status, so
    // there is nothing for the 404 test to read.
    let error = BootstrapError::PackageJsonMissingMain {
        module: "@edge-toolkit/et-ws-math1".to_string(),
    };

    assert!(!is_module_file_missing(&error));
}
