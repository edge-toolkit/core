//! Standalone supervisor builds negotiate TLS for secure WebSocket endpoints.
#![cfg(test)]

use std::time::Duration;

use tokio::io::AsyncReadExt as _;

/// Require a real TLS `ClientHello` from the supervisor's independently enabled secure transport.
#[tokio::test]
async fn secure_websocket_sends_a_tls_handshake_instead_of_rejecting_the_scheme() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let endpoint = format!("wss://{}/ws", listener.local_addr().unwrap());
    let client = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(endpoint))
            .await
            .unwrap()
    });
    let (mut peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut record_type = [0_u8; 1];
    let _read = tokio::time::timeout(Duration::from_secs(10), peer.read_exact(&mut record_type))
        .await
        .unwrap()
        .unwrap();
    // TLS handshake records start with 0x16. Without TLS compiled in, the client closes before sending a record.
    assert_eq!(record_type, [0x16_u8]);
    drop(peer);
    // This peer deliberately stops after ClientHello; it cannot complete a secure WebSocket connection.
    let _handshake_error = client.await.unwrap().unwrap_err();
}
