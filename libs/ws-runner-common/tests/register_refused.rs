//! Covers which upgrade answers `connect_and_register` retries: a refusal ends it, any other status does not.
#![cfg(test)]

use std::sync::atomic::Ordering;
use std::time::Duration;

use et_test_helpers::http_answering;
use et_ws_runner_common::{ConnectError, connect_and_register};
use pretty_assertions::assert_eq;

#[tokio::test]
async fn a_refused_upgrade_ends_the_wait_at_once() {
    let (port, requests) = http_answering("403 Forbidden");
    let url = format!("ws://127.0.0.1:{port}/ws");

    let err = tokio::time::timeout(Duration::from_secs(10), connect_and_register(&url, None, None))
        .await
        .unwrap()
        .unwrap_err();

    assert!(matches!(err, ConnectError::Refused), "got {err:?}");
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn another_error_status_is_retried_until_the_budget_is_spent() {
    let (port, requests) = http_answering("503 Service Unavailable");
    let url = format!("ws://127.0.0.1:{port}/ws");

    let err = connect_and_register(&url, None, Some(Duration::from_secs(1)))
        .await
        .unwrap_err();

    assert!(matches!(err, ConnectError::WebSocket(_)), "got {err:?}");
    assert!(requests.load(Ordering::SeqCst) > 1, "a 503 was not retried");
}
