//! End-to-end tests for the `/websockify` WebSocket-to-TCP relay.
//!
//! These stand in for the browser: a real WebSocket client (tokio-tungstenite) connects to the relay, and the
//! relay bridges to a plain TCP target we control. That is exactly the shape of the browser path -- webR's
//! Emscripten libcurl opens a WebSocket and writes/reads raw TCP bytes -- minus the Emscripten framing, which is
//! ordinary WebSocket binary. Each target below (echo, a minimal HTTP responder, a closing socket, an
//! unreachable port) pins one behaviour the browser depends on.

#![cfg(test)]

use std::net::SocketAddr;
use std::time::Duration;

use actix_web::{App, HttpServer};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Start the relay app bridging to `target`, on an ephemeral port; return that port once it accepts.
#[expect(
    clippy::future_not_send,
    reason = "test helper; the actix App factory is !Send and runs on the current-thread test runtime"
)]
async fn start_relay(target: SocketAddr) -> u16 {
    let server = HttpServer::new(move || App::new().configure(|cfg| et_websockify_service::configure(cfg, target)))
        .workers(1)
        .bind(("127.0.0.1", 0))
        .unwrap();
    let port = server.addrs()[0].port();
    let _server = actix_web::rt::spawn(server.run());
    for _ in 0_u32..100 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    port
}

/// A TCP target that echoes every byte back -- exercises both relay directions.
async fn start_echo_target() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _task = actix_web::rt::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let _conn = actix_web::rt::spawn(async move {
                let mut buf = [0_u8; 4096];
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if sock.write_all(&buf[..read]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}

/// A TCP target that answers any request with a fixed HTTP/1.1 200 -- what libcurl over the tunnel expects.
#[expect(clippy::single_call_fn, reason = "test target used by one case")]
async fn start_http_target() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _task = actix_web::rt::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let _conn = actix_web::rt::spawn(async move {
                let mut buf = [0_u8; 4096];
                let _read = sock.read(&mut buf).await;
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\npong";
                let _written = sock.write_all(response.as_bytes()).await;
                let _shutdown = sock.shutdown().await;
            });
        }
    });
    addr
}

/// A TCP target that accepts then immediately closes -- the relay must propagate that as a WebSocket close.
#[expect(clippy::single_call_fn, reason = "test target used by one case")]
async fn start_closing_target() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _task = actix_web::rt::spawn(async move {
        while let Ok((_sock, _)) = listener.accept().await {
            // `_sock` drops at the end of the loop body, closing the connection immediately.
        }
    });
    addr
}

/// Reserve then release a port so nothing is listening on it -- an unreachable relay target.
#[expect(clippy::single_call_fn, reason = "test target used by one case")]
async fn unreachable_target() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    listener.local_addr().unwrap()
}

async fn open_ws(port: u16) -> WsStream {
    let (ws, _response) = connect_async(format!("ws://127.0.0.1:{port}/websockify"))
        .await
        .unwrap();
    ws
}

/// Accumulate relayed bytes until at least `want` have arrived (or the socket closes), with a timeout.
async fn recv_bytes(ws: &mut WsStream, want: usize) -> Vec<u8> {
    let mut acc = Vec::new();
    let collected = tokio::time::timeout(Duration::from_secs(10), async {
        while acc.len() < want {
            match ws.next().await {
                Some(Ok(Message::Binary(bytes))) => acc.extend_from_slice(&bytes),
                Some(Ok(Message::Text(text))) => acc.extend_from_slice(text.as_bytes()),
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(err)) => panic!("websocket error: {err}"),
            }
        }
    })
    .await;
    collected.unwrap();
    acc
}

/// Return true if the relay closes the WebSocket within the timeout (a `Close` frame, end of stream, or error).
async fn closed_within(ws: &mut WsStream, secs: u64) -> bool {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return true,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .unwrap_or(false)
}

#[actix_web::test]
async fn relay_echoes_bytes_both_directions() {
    let port = start_relay(start_echo_target().await).await;
    let mut ws = open_ws(port).await;

    ws.send(Message::Binary(b"hello relay".to_vec())).await.unwrap();
    let echoed = recv_bytes(&mut ws, b"hello relay".len()).await;

    assert_eq!(echoed, b"hello relay");
}

#[actix_web::test]
async fn relay_carries_a_full_http_get() {
    let port = start_relay(start_http_target().await).await;
    let mut ws = open_ws(port).await;

    // Exactly what libcurl/httr2 would write over the tunnel.
    let request = "GET /storage/agent/data.txt HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n";
    ws.send(Message::Binary(request.as_bytes().to_vec())).await.unwrap();

    let response = String::from_utf8(recv_bytes(&mut ws, 48).await).unwrap();
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "unexpected response: {response}"
    );
    assert!(response.contains("pong"), "response missing body: {response}");
}

#[actix_web::test]
async fn relay_handles_a_large_chunked_payload() {
    let port = start_relay(start_echo_target().await).await;
    let mut ws = open_ws(port).await;

    // Larger than the relay's 16 KiB read buffer, so both directions must span multiple reads/frames.
    let payload = vec![0xAB_u8; 200 * 1024];
    ws.send(Message::Binary(payload.clone())).await.unwrap();
    let echoed = recv_bytes(&mut ws, payload.len()).await;

    assert_eq!(echoed.len(), payload.len());
    assert_eq!(echoed, payload);
}

#[actix_web::test]
async fn relay_closes_when_target_closes() {
    let port = start_relay(start_closing_target().await).await;
    let mut ws = open_ws(port).await;
    // Send a (non-SOCKS5) byte so the relay sniffs the protocol and connects to the target, which closed.
    ws.send(Message::Binary(b"GET / HTTP/1.1\r\n\r\n".to_vec()))
        .await
        .unwrap();

    assert!(
        closed_within(&mut ws, 10).await,
        "relay did not close after target closed"
    );
}

#[actix_web::test]
async fn relay_closes_when_target_unreachable() {
    let port = start_relay(unreachable_target().await).await;
    let mut ws = open_ws(port).await;
    // The relay only connects to the target after the client's first byte; send one so it tries (and fails).
    ws.send(Message::Binary(b"GET / HTTP/1.1\r\n\r\n".to_vec()))
        .await
        .unwrap();

    assert!(
        closed_within(&mut ws, 10).await,
        "relay did not close when target was unreachable"
    );
}

#[actix_web::test]
async fn relay_accepts_an_appended_target_path() {
    // Emscripten may append the connect target to the configured URL: ws://host/websockify/<addr>:<port>.
    // The relay must still match and bridge to its own fixed target, ignoring the suffix.
    let port = start_relay(start_echo_target().await).await;
    let (mut ws, _response) = connect_async(format!("ws://127.0.0.1:{port}/websockify/127.0.0.1:8080"))
        .await
        .unwrap();

    ws.send(Message::Binary(b"suffixed".to_vec())).await.unwrap();
    let echoed = recv_bytes(&mut ws, b"suffixed".len()).await;

    assert_eq!(echoed, b"suffixed");
}

/// Do the SOCKS5 no-auth method negotiation, send `request`, and return the open socket + the CONNECT reply.
async fn socks5_connect(port: u16, request: &[u8]) -> (WsStream, Vec<u8>) {
    let mut ws = open_ws(port).await;
    // Method selection: version 5, one method, "no authentication".
    ws.send(Message::Binary(vec![0x05, 0x01, 0x00])).await.unwrap();
    assert_eq!(
        recv_bytes(&mut ws, 2).await,
        vec![0x05, 0x00],
        "SOCKS5 method-selection reply"
    );
    ws.send(Message::Binary(request.to_vec())).await.unwrap();
    let reply = recv_bytes(&mut ws, 10).await;
    (ws, reply)
}

#[actix_web::test]
async fn relay_socks5_connect_to_loopback_bridges() {
    let port = start_relay(start_echo_target().await).await;
    // CONNECT 127.0.0.1:80 -- loopback, so accepted; the relay bridges to its own (echo) target.
    let request = [0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0x00, 0x50];
    let (mut ws, reply) = socks5_connect(port, &request).await;
    assert_eq!(
        reply.get(0..2),
        Some([0x05, 0x00].as_slice()),
        "expected SOCKS5 success, got {reply:?}"
    );

    // Tunnel is live -> bytes echo back through the bridged target.
    ws.send(Message::Binary(b"socks-hi".to_vec())).await.unwrap();
    assert_eq!(recv_bytes(&mut ws, b"socks-hi".len()).await, b"socks-hi");
}

#[actix_web::test]
async fn relay_socks5_refuses_non_loopback_ipv4() {
    let port = start_relay(start_echo_target().await).await;
    // CONNECT 8.8.8.8:443 -- not loopback, must be refused (REP=2) and never reach the target.
    let request = [0x05, 0x01, 0x00, 0x01, 8, 8, 8, 8, 0x01, 0xBB];
    let (_ws, reply) = socks5_connect(port, &request).await;
    assert_eq!(
        reply.get(1),
        Some(&0x02),
        "expected SOCKS5 refusal (REP=2), got {reply:?}"
    );
}

#[actix_web::test]
async fn relay_socks5_refuses_external_domain() {
    let port = start_relay(start_echo_target().await).await;
    // CONNECT get-ws-proxy.r-universe.dev:443 -- the exact stray probe webR's curl makes; must be refused.
    let host = b"get-ws-proxy.r-universe.dev";
    let mut request = vec![0x05, 0x01, 0x00, 0x03, u8::try_from(host.len()).unwrap()];
    request.extend_from_slice(host);
    request.extend_from_slice(&[0x01, 0xBB]);
    let (_ws, reply) = socks5_connect(port, &request).await;
    assert_eq!(
        reply.get(1),
        Some(&0x02),
        "expected SOCKS5 refusal for external domain, got {reply:?}"
    );
}

#[actix_web::test]
async fn relay_echoes_the_binary_subprotocol() {
    let port = start_relay(start_echo_target().await).await;

    // Emscripten offers `binary`; the server must echo it so the browser gets unencoded binary frames.
    let mut request = format!("ws://127.0.0.1:{port}/websockify")
        .into_client_request()
        .unwrap();
    let _prev = request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("binary"));
    let (_ws, response) = connect_async(request).await.unwrap();

    let negotiated = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok());
    assert_eq!(negotiated, Some("binary"));
}
