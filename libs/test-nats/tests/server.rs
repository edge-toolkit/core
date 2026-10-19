//! A started broker listens on the port its URL names, and is gone once dropped.
#![cfg(test)]

use std::net::TcpStream;

use et_test_nats::NatsServer;

#[test]
fn a_started_broker_accepts_connections_at_its_url() {
    let mut server = NatsServer::start();
    let url = server.url();
    let address = url.strip_prefix("nats://").unwrap();

    assert!(server.is_running());
    let _accepted = TcpStream::connect(address).unwrap();

    drop(server);
    let _refused = TcpStream::connect(address).unwrap_err();
}
