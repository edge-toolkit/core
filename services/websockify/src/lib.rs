#![cfg_attr(doc, doc = include_str!("../README.md"))]

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use actix_web::http::header::{HeaderValue, SEC_WEBSOCKET_PROTOCOL};
use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_ws::{AggregatedMessage, AggregatedMessageStream, MessageStream, Session};
use bytes::Bytes;
use futures_util::StreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

/// Loopback TCP endpoint every `/websockify` connection is bridged to.
///
/// Set by the server in [`configure`], never by the client -- this is what keeps the relay from being an open
/// proxy: a browser chooses neither the host nor the port.
#[derive(Clone, Copy)]
struct RelayTarget(SocketAddr);

/// Register the `/websockify` relay route, bridging every connection to `target`.
///
/// `target` is expected to be a loopback address -- the ws-server passes its own plain-HTTP port. Register this
/// before any catch-all `Files::new("/")` mount so the explicit route wins.
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "`{tail:.*}` is an actix-web route path pattern, not a format string"
)]
pub fn configure(cfg: &mut web::ServiceConfig, target: SocketAddr) {
    // Match a trailing path too: Emscripten may append the connect target to the configured WebSocket URL
    // (e.g. `/websockify/127.0.0.1:8080`). The suffix is ignored -- the relay always bridges to the fixed
    // server-configured target -- so it stays SSRF-safe regardless of what the client puts there.
    let _routed = cfg
        .app_data(web::Data::new(RelayTarget(target)))
        .route("/websockify{tail:.*}", web::get().to(relay_handler));
}

/// Bytes copied per TCP->WebSocket read. 16 KiB is a typical socket read; large reads just loop.
const RELAY_BUF: usize = 16 * 1024;

/// Upper bound on a single inbound WebSocket frame / aggregated message the relay will accept.
///
/// Emscripten sends the tunnelled TCP stream as many small binary messages, so this is only a safety ceiling;
/// it is set well above any realistic single send (and above actix-ws's smaller defaults, which would otherwise
/// reject a large upload frame and tear the relay down).
const RELAY_MAX_MESSAGE: usize = 8 * 1024 * 1024;

/// How many bytes of a relayed chunk to show in the debug preview log line.
const PREVIEW_BYTES: usize = 256;

// SOCKS5 (RFC 1928) constants for the minimal proxy the relay speaks.
const SOCKS5_VERSION: u8 = 0x05;
const SOCKS5_CMD_CONNECT: u8 = 0x01;
const SOCKS5_ATYP_IPV4: u8 = 0x01;
const SOCKS5_ATYP_DOMAIN: u8 = 0x03;
const SOCKS5_ATYP_IPV6: u8 = 0x04;
/// Method-selection reply: version 5, "no authentication required".
const SOCKS5_NO_AUTH: [u8; 2] = [SOCKS5_VERSION, 0x00];
/// CONNECT reply, succeeded: VER, REP=0, RSV, ATYP=IPv4, BND.ADDR=0.0.0.0, BND.PORT=0.
const SOCKS5_REPLY_OK: [u8; 10] = [SOCKS5_VERSION, 0x00, 0x00, SOCKS5_ATYP_IPV4, 0, 0, 0, 0, 0, 0];
/// CONNECT reply, connection not allowed by ruleset (REP=2).
const SOCKS5_REPLY_REFUSED: [u8; 10] = [SOCKS5_VERSION, 0x02, 0x00, SOCKS5_ATYP_IPV4, 0, 0, 0, 0, 0, 0];

/// Render the first [`PREVIEW_BYTES`] of a relayed chunk as an escaped string for logging.
///
/// Escaping makes CR/LF and any non-printable bytes visible, so a malformed HTTP request line or header (or an
/// unexpectedly base64-encoded frame) is obvious in the log rather than mangling the terminal.
fn preview(bytes: &[u8]) -> String {
    let end = bytes.len().min(PREVIEW_BYTES);
    String::from_utf8_lossy(bytes.get(..end).unwrap_or_default())
        .escape_default()
        .to_string()
}

#[expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "actix handler: HttpRequest/Payload are Rc-backed and !Send; registered once in configure"
)]
async fn relay_handler(
    req: HttpRequest,
    body: web::Payload,
    target: web::Data<RelayTarget>,
) -> Result<HttpResponse, Error> {
    // Emscripten offers the `binary` subprotocol; echo it so the browser WebSocket sees a negotiated protocol
    // and delivers unencoded binary frames rather than falling back to base64-encoded text.
    let offered = req
        .headers()
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let offers_binary = offered
        .as_deref()
        .is_some_and(|protocols| protocols.split(',').any(|protocol| protocol.trim() == "binary"));
    info!(
        "upgrade on {} -> target {} (offered subprotocols: {:?}, echoing binary: {offers_binary})",
        req.path(),
        target.0,
        offered.as_deref().unwrap_or("<none>"),
    );

    let (mut response, session, stream) = actix_ws::handle(&req, body)?;
    if offers_binary {
        let _prev = response
            .headers_mut()
            .insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static("binary"));
    }

    let target = target.0;
    let _join = actix_web::rt::spawn(async move {
        relay(session, stream, target).await;
    });
    Ok(response)
}

/// Read from the WebSocket into `buf` until it holds at least `need` bytes; false if the socket closed/errored.
#[expect(
    clippy::future_not_send,
    reason = "actix-ws Session/stream are Rc-backed and !Send; runs on the current-thread actix runtime"
)]
async fn fill(stream: &mut AggregatedMessageStream, session: &mut Session, buf: &mut Vec<u8>, need: usize) -> bool {
    while buf.len() < need {
        match stream.next().await {
            Some(Ok(AggregatedMessage::Binary(bytes))) => buf.extend_from_slice(bytes.as_ref()),
            Some(Ok(AggregatedMessage::Text(text))) => buf.extend_from_slice(text.as_bytes()),
            Some(Ok(AggregatedMessage::Ping(ping))) => {
                let _pong = session.pong(&ping).await;
            }
            Some(Ok(AggregatedMessage::Pong(_))) => {}
            _ => return false,
        }
    }
    true
}

/// Perform the minimal SOCKS5 server handshake over the WebSocket.
///
/// Returns `Some(true)` if the client issued a CONNECT to a loopback target (success reply sent; `buf` left
/// holding any leftover tunnelled bytes), `Some(false)` if the request was refused (error reply sent), or `None`
/// if the socket closed mid-handshake. Only no-auth CONNECT to a loopback address is accepted; the relay then
/// bridges to its own fixed target, so the requested port is not otherwise used.
#[expect(
    clippy::cognitive_complexity,
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "!Send actix-ws types; one call site; an awaited read per SOCKS5 field, each with an early return"
)]
async fn socks5_handshake(
    stream: &mut AggregatedMessageStream,
    session: &mut Session,
    buf: &mut Vec<u8>,
) -> Option<bool> {
    // Method selection: VER, NMETHODS, METHODS...
    if !fill(stream, session, buf, 2).await {
        return None;
    }
    let nmethods = usize::from(*buf.get(1)?);
    let select_len = 2_usize.saturating_add(nmethods);
    if !fill(stream, session, buf, select_len).await {
        return None;
    }
    // Discard the consumed method-selection bytes, keeping any tail already buffered.
    *buf = buf.split_off(select_len);
    session.binary(Bytes::from_static(&SOCKS5_NO_AUTH)).await.ok()?;

    // Request: VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT.
    if !fill(stream, session, buf, 4).await {
        return None;
    }
    let command = *buf.get(1)?;
    let atyp = *buf.get(3)?;
    let (loopback, request_len) = match atyp {
        SOCKS5_ATYP_IPV4 => {
            if !fill(stream, session, buf, 10).await {
                return None;
            }
            let octets: [u8; 4] = buf.get(4..8)?.try_into().ok()?;
            (Ipv4Addr::from(octets).is_loopback(), 10)
        }
        SOCKS5_ATYP_IPV6 => {
            if !fill(stream, session, buf, 22).await {
                return None;
            }
            let octets: [u8; 16] = buf.get(4..20)?.try_into().ok()?;
            (Ipv6Addr::from(octets).is_loopback(), 22)
        }
        SOCKS5_ATYP_DOMAIN => {
            if !fill(stream, session, buf, 5).await {
                return None;
            }
            let name_len = usize::from(*buf.get(4)?);
            let request_len = 5_usize.saturating_add(name_len).saturating_add(2);
            if !fill(stream, session, buf, request_len).await {
                return None;
            }
            let name_end = 5_usize.saturating_add(name_len);
            let name = String::from_utf8_lossy(buf.get(5..name_end)?).into_owned();
            (matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1"), request_len)
        }
        _ => {
            session.binary(Bytes::from_static(&SOCKS5_REPLY_REFUSED)).await.ok()?;
            return Some(false);
        }
    };

    if command == SOCKS5_CMD_CONNECT && loopback {
        session.binary(Bytes::from_static(&SOCKS5_REPLY_OK)).await.ok()?;
        // Discard the consumed request bytes, keeping any tunnelled data the client already sent after it.
        *buf = buf.split_off(request_len);
        Some(true)
    } else {
        session.binary(Bytes::from_static(&SOCKS5_REPLY_REFUSED)).await.ok()?;
        Some(false)
    }
}

#[expect(
    clippy::cognitive_complexity,
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "Rc-backed !Send actix-ws types; called once from relay_handler; score is tracing-macro expansion"
)]
async fn relay(mut session: Session, stream: MessageStream, target: SocketAddr) {
    let mut stream = stream
        .max_frame_size(RELAY_MAX_MESSAGE)
        .aggregate_continuations()
        .max_continuation_size(RELAY_MAX_MESSAGE);
    let mut head: Vec<u8> = Vec::new();
    if !accept_client(&mut stream, &mut session, &mut head, target).await {
        let _closed = session.close(None).await;
        return;
    }

    let tcp = match TcpStream::connect(target).await {
        Ok(tcp) => tcp,
        Err(err) => {
            warn!("cannot reach relay target {target}: {err}");
            let _closed = session.close(None).await;
            return;
        }
    };
    info!("bridged to target {target}");
    bridge(session, stream, tcp, head).await;
}

/// Recognise the client from its first byte and complete any handshake it needs, reporting whether to bridge it.
///
/// A SOCKS5 client (webR's curl proxy) must ask for a loopback CONNECT; anything else is a direct byte stream, whose
/// first byte stays in `head` to be forwarded. The caller closes the session when this returns `false`.
#[expect(
    clippy::cognitive_complexity,
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "Rc-backed !Send actix-ws types; the client-recognition step of relay; score is tracing-macro expansion"
)]
async fn accept_client(
    stream: &mut AggregatedMessageStream,
    session: &mut Session,
    head: &mut Vec<u8>,
    target: SocketAddr,
) -> bool {
    if !fill(stream, session, head, 1).await {
        return false;
    }
    if head.first() != Some(&SOCKS5_VERSION) {
        info!("direct (non-SOCKS5) client; bridging to {target}");
        return true;
    }
    match socks5_handshake(stream, session, head).await {
        Some(true) => {
            info!("SOCKS5 CONNECT to loopback accepted; bridging to {target}");
            true
        }
        Some(false) => {
            info!("refused SOCKS5 CONNECT (non-loopback target or unsupported command)");
            false
        }
        None => false,
    }
}

#[expect(
    clippy::cognitive_complexity,
    clippy::future_not_send,
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    clippy::too_many_lines,
    reason = "Rc-backed !Send actix-ws types; select! uses % internally; one call site; one select! loop is the bridge"
)]
async fn bridge(mut session: Session, mut stream: AggregatedMessageStream, tcp: TcpStream, head: Vec<u8>) {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let mut to_target = 0_usize;
    let mut to_client = 0_usize;

    // Forward any bytes already read during protocol detection (the HTTP request start, or SOCKS5 tunnel data).
    if !head.is_empty() {
        info!("first client->target bytes ({} bytes): {}", head.len(), preview(&head));
        to_target = head.len();
        if tcp_write.write_all(&head).await.is_err() {
            let _closed = session.close(None).await;
            return;
        }
    }

    let mut buf = vec![0_u8; RELAY_BUF];
    loop {
        tokio::select! {
            inbound = stream.next() => match inbound {
                // Browser -> TCP: forward the raw bytes of each message to the target socket.
                Some(Ok(AggregatedMessage::Binary(bytes))) => {
                    to_target = to_target.saturating_add(bytes.len());
                    if tcp_write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Some(Ok(AggregatedMessage::Text(text))) => {
                    to_target = to_target.saturating_add(text.len());
                    if tcp_write.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Some(Ok(AggregatedMessage::Ping(ping))) => {
                    let _pong = session.pong(&ping).await;
                }
                Some(Ok(AggregatedMessage::Pong(_))) => {}
                Some(Ok(AggregatedMessage::Close(reason))) => {
                    info!("client closed the websocket ({reason:?})");
                    break;
                }
                None => break,
                Some(Err(err)) => {
                    debug!("websocket receive error: {err}");
                    break;
                }
            },
            outbound = tcp_read.read(&mut buf) => match outbound {
                // TCP -> browser: 0 bytes means the target closed; anything else is a binary frame.
                Ok(0) => {
                    debug!("target closed the connection");
                    break;
                }
                Err(err) => {
                    debug!("target read failed: {err}");
                    break;
                }
                Ok(read) => {
                    // `read` is always <= buf.len() per AsyncReadExt::read, so get(..read) is always Some.
                    let Some(chunk) = buf.get(..read) else { break };
                    if to_client == 0 {
                        info!("first target->client chunk ({read} bytes): {}", preview(chunk));
                    }
                    to_client = to_client.saturating_add(read);
                    if session.binary(Bytes::copy_from_slice(chunk)).await.is_err() {
                        break;
                    }
                }
            },
        }
    }

    let _shutdown = tcp_write.shutdown().await;
    let _closed = session.close(None).await;
    info!("closed (client->target {to_target} bytes, target->client {to_client} bytes)");
}
