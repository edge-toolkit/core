//! Exercise the math1 fake-agent driver's success and error paths against an in-process server.
//!
//! The runner integration suites only ever see the happy path (a real module answers), so this
//! file drives [`et_ws_test_server::math1`] directly: a second ws client plays the module's role
//! by writing `math1-output.json` shapes straight into its own storage bucket, and the driver's
//! transport, timeout, parse, and verification branches are asserted one by one.

#![cfg(test)]

use std::time::Duration;

use et_ws_test_server::math1::{
    MATH1_EXPECTED_BIAS, MATH1_EXPECTED_WEIGHT, MATH1_OUTPUT_FILENAME, Math1Error, drive_math1_exchange,
    verify_math1_model,
};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

/// Budget generous enough for the driver to see the peer and its pre-written output.
const EXCHANGE_BUDGET: Duration = Duration::from_secs(30);

#[test]
fn verify_accepts_the_expected_model() {
    verify_math1_model(MATH1_EXPECTED_WEIGHT, MATH1_EXPECTED_BIAS).unwrap();
}

#[test]
fn verify_rejects_a_wrong_weight() {
    let err = verify_math1_model(MATH1_EXPECTED_WEIGHT + 1.0, MATH1_EXPECTED_BIAS).unwrap_err();
    assert!(err.to_string().contains("weight"), "unexpected error: {err}");
}

#[test]
fn verify_rejects_a_wrong_bias() {
    let err = verify_math1_model(MATH1_EXPECTED_WEIGHT, MATH1_EXPECTED_BIAS + 1.0).unwrap_err();
    assert!(err.to_string().contains("bias"), "unexpected error: {err}");
}

/// An unreachable server surfaces as the boxed transport variant (covers the `From` impl).
#[tokio::test(flavor = "current_thread")]
async fn unreachable_server_is_a_transport_error() {
    let err = drive_math1_exchange("ws://127.0.0.1:9/ws", std::env::temp_dir().as_path(), EXCHANGE_BUDGET)
        .await
        .unwrap_err();
    assert!(matches!(err, Math1Error::Transport(_)), "unexpected error: {err}");
}

/// A server that closes the websocket cleanly mid-exchange surfaces as the socket-closed error.
#[tokio::test(flavor = "current_thread")]
async fn server_close_is_a_protocol_error() {
    let ws_url = accept_one_connection_then(true).await;
    let err = drive_math1_exchange(&ws_url, std::env::temp_dir().as_path(), EXCHANGE_BUDGET)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("socket closed"),
        "expected the socket-closed protocol error, got: {err}"
    );
}

/// A server that drops the TCP stream without a close handshake surfaces as a transport error.
#[tokio::test(flavor = "current_thread")]
async fn abrupt_server_drop_is_a_transport_error() {
    let ws_url = accept_one_connection_then(false).await;
    let err = drive_math1_exchange(&ws_url, std::env::temp_dir().as_path(), EXCHANGE_BUDGET)
        .await
        .unwrap_err();
    assert!(matches!(err, Math1Error::Transport(_)), "unexpected error: {err}");
}

/// Accept exactly one ws connection, consume its first frame, then close it (gracefully or not).
///
/// Returns the `ws://` URL to hand to the driver. The close style picks which driver arm trips:
/// a clean close handshake ends the stream (`None`), an abrupt TCP drop yields a protocol `Err`.
async fn accept_one_connection_then(close_gracefully: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        // Consume the driver's et-connect frame so its send completes before the teardown.
        let _frame = socket.next().await;
        if close_gracefully {
            socket.close(None).await.unwrap();
            // Hold the stream open long enough for the close handshake to reach the driver.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Dropping the socket here tears the TCP stream down without a close handshake.
    });
    format!("ws://{addr}/ws")
}

/// With no module ever answering, the driver keeps re-broadcasting until the budget expires.
#[tokio::test(flavor = "current_thread")]
async fn times_out_when_no_module_answers() {
    let server = et_ws_test_server::start();
    let err = drive_math1_exchange(&server.ws_url, server.storage_dir.path(), Duration::from_millis(600))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("timed out"),
        "expected the timeout protocol error, got: {err}"
    );
}

/// Happy path: a peer's stored output is found, parsed, and passes verification.
///
/// The peer also relays noise (a foreign text frame and a binary frame) mid-exchange, so the
/// driver's ignore-arms for non-protocol traffic are exercised alongside the success path.
#[tokio::test(flavor = "current_thread")]
async fn reads_and_verifies_a_peer_output() {
    let server = et_ws_test_server::start();
    // The peer plays the module: it registers on the hub and its bucket carries a valid output.
    let (mut peer, peer_id) = et_ws_test_server::connect_agent(&server.ws_url).await;
    let storage_dir = server.storage_dir.path().to_path_buf();
    let output = format!(r#"{{"module":"t","weight":{MATH1_EXPECTED_WEIGHT},"bias":{MATH1_EXPECTED_BIAS}}}"#);
    let noise_then_output = tokio::spawn(async move {
        // Let the fake agent connect and start draining, then relay noise before the output lands.
        tokio::time::sleep(Duration::from_millis(600)).await;
        peer.send(Message::Text(r#"{"type":"noise"}"#.to_string()))
            .await
            .unwrap();
        peer.send(Message::Binary(vec![1, 2, 3])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let bucket = storage_dir.join(&peer_id);
        fs_err::create_dir_all(&bucket).unwrap();
        fs_err::write(bucket.join(MATH1_OUTPUT_FILENAME), output).unwrap();
        peer // keep the peer registered until the exchange resolves
    });
    let (weight, bias) = drive_math1_exchange(&server.ws_url, server.storage_dir.path(), EXCHANGE_BUDGET)
        .await
        .unwrap();
    verify_math1_model(weight, bias).unwrap();
    let _peer = noise_then_output.await.unwrap();
}

/// A non-JSON output file surfaces as the JSON variant.
#[tokio::test(flavor = "current_thread")]
async fn malformed_output_is_a_json_error() {
    let server = et_ws_test_server::start();
    let (_peer, peer_id) = et_ws_test_server::connect_agent(&server.ws_url).await;
    write_peer_output(&server, &peer_id, "not json");
    let err = drive_math1_exchange(&server.ws_url, server.storage_dir.path(), EXCHANGE_BUDGET)
        .await
        .unwrap_err();
    assert!(matches!(err, Math1Error::Json(_)), "unexpected error: {err}");
}

/// Output JSON without the model fields names the first missing field.
#[tokio::test(flavor = "current_thread")]
async fn output_without_weight_is_a_protocol_error() {
    let server = et_ws_test_server::start();
    let (_peer, peer_id) = et_ws_test_server::connect_agent(&server.ws_url).await;
    write_peer_output(&server, &peer_id, r#"{"module":"t"}"#);
    let err = drive_math1_exchange(&server.ws_url, server.storage_dir.path(), EXCHANGE_BUDGET)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("weight"), "unexpected error: {err}");
}

/// Output JSON with a weight but no bias trips the second field check.
#[tokio::test(flavor = "current_thread")]
async fn output_without_bias_is_a_protocol_error() {
    let server = et_ws_test_server::start();
    let (_peer, peer_id) = et_ws_test_server::connect_agent(&server.ws_url).await;
    write_peer_output(&server, &peer_id, r#"{"module":"t","weight":1.0}"#);
    let err = drive_math1_exchange(&server.ws_url, server.storage_dir.path(), EXCHANGE_BUDGET)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("bias"), "unexpected error: {err}");
}

/// Drop `content` into the peer's storage bucket as its `math1-output.json`.
fn write_peer_output(server: &et_ws_test_server::TestServer, peer_id: &str, content: &str) {
    let bucket = server.storage_dir.path().join(peer_id);
    fs_err::create_dir_all(&bucket).unwrap();
    fs_err::write(bucket.join(MATH1_OUTPUT_FILENAME), content).unwrap();
}
