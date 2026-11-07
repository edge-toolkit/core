//! A board's guest reaching a hub on another machine through `board::net`, over real sockets on this host.
//!
//! The sockets are tokio's, read and written as a board's are, and the hub is either the in-process ws-server, with
//! its storage, or one scripted here a frame at a time.
#![cfg(test)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

// The host's critical section, which the board's queues lock under, as a board's own does on the board.
use critical_section as _;
use et_ws_wasi_runner::bindings::wasi::logging::logging::Level;
use et_ws_wasi_runner::board::net::{NetError, NetLink, NetTransport, drive};
use et_ws_wasi_runner::board::net_tokio::TokioConnector;
use et_ws_wasi_runner::board::{Console, MemoryBudget, Platform, Transport as _, run};
use et_ws_wasi_runner::convert::Frame;
use et_ws_wasi_runner::engine::{NATIVE_PULLEY, board_engine, precompile};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;

/// When the test process started, which the board's clock counts from.
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// How long a test waits on the hub before failing rather than hanging.
const PATIENCE: Duration = Duration::from_secs(20);

/// The upgrade a hub answers a websocket's request with.
const UPGRADED: &str = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";

/// A board's clock, this host's, and its log, kept in a vector.
#[derive(Clone, Default)]
struct Board(Arc<Mutex<Vec<String>>>);

impl Platform for Board {
    fn now(&self) -> u64 {
        u64::try_from(STARTED.elapsed().as_nanos()).unwrap()
    }

    async fn sleep_until(&self, deadline: u64) {
        tokio::time::sleep(Duration::from_nanos(deadline.saturating_sub(self.now()))).await;
    }

    fn log(&self, _level: Level, _context: &str, message: &str) {
        self.0.lock().unwrap().push(message.to_owned());
    }

    fn write(&self, _console: Console, bytes: &[u8]) {
        self.0.lock().unwrap().push(String::from_utf8_lossy(bytes).into_owned());
    }
}

/// A transport to the hub at `address`, its sockets driven on this thread until the test ends.
fn transport_to(address: SocketAddr, board: Board) -> NetTransport<Board> {
    let link: &'static NetLink = Box::leak(Box::new(NetLink::idle()));
    let host = address.to_string();
    let driving = board.clone();
    let dial = TokioConnector::new(address);
    let _driving = tokio::task::spawn_local(async move {
        drive(link, dial, dial, &host, driving).await;
    });
    NetTransport::new(link, board)
}

#[tokio::test(flavor = "current_thread")]
async fn a_guest_round_trips_a_value_through_a_hub_elsewhere() {
    let server = et_ws_test_server::start();
    let address: SocketAddr = server.base_url.trim_start_matches("http://").parse().unwrap();
    let board = Board::default();
    LocalSet::new()
        .run_until(async {
            let transport = transport_to(address, board.clone());
            let built =
                edge_toolkit::config::get_project_root().join("modules/wasi/wasi-data1/pkg/et_ws_wasi_data1.wasm");
            let component = precompile(NATIVE_PULLEY, &fs_err::read(built).unwrap()).unwrap();
            let engine = board_engine().unwrap();
            let ran = run(
                &engine,
                &component,
                board.clone(),
                transport,
                MemoryBudget::new(usize::MAX),
            );
            tokio::time::timeout(PATIENCE, ran).await.unwrap().unwrap();
        })
        .await;
    let lines = board.0.lock().unwrap().clone();
    assert!(lines.iter().any(|line| line.contains("VERIFICATION SUCCESS")));
    // What the guest stored is in the hub's storage, not the board's.
    let buckets = fs_err::read_dir(server.storage_dir.path()).unwrap().count();
    assert!(buckets > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn storage_answers_nothing_stored_and_refuses_a_stranger() {
    let server = et_ws_test_server::start();
    let address: SocketAddr = server.base_url.trim_start_matches("http://").parse().unwrap();
    LocalSet::new()
        .run_until(async {
            let mut transport = transport_to(address, Board::default());
            assert_eq!(transport.load("nobody", "a b.json").await, Ok(None));
            assert_eq!(
                transport.store("nobody", "x", vec![1]).await,
                Err(NetError::Status(404))
            );
            // Asked of a socket with no websocket open, which the driving task has nothing to do with.
            assert_eq!(
                transport.send(Frame::Text(String::default())).await,
                Err(NetError::Closed)
            );
            transport.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_hub_that_cannot_be_reached_is_said_to_be() {
    let unused = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = unused.local_addr().unwrap();
    drop(unused);
    LocalSet::new()
        .run_until(async {
            let mut transport = transport_to(address, Board::default());
            let refused = transport.connect().await.unwrap_err();
            assert!(refused.to_string().starts_with("could not reach the hub: "));
            assert!(matches!(
                transport.load("bucket", "key").await,
                Err(NetError::Connect(_))
            ));
        })
        .await;
}

/// A hub answering one connection as `script` says, after the upgrade `upgrade` answers it with.
async fn scripted<S, F>(upgrade: &'static str, script: S) -> SocketAddr
where
    S: FnOnce(TcpStream) -> F + 'static,
    F: Future<Output = ()> + 'static,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let _serving = tokio::task::spawn_local(async move {
        let (mut socket, _peer) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(upgrade.as_bytes()).await.unwrap();
        script(socket).await;
    });
    address
}

/// An unmasked frame of `opcode`, final unless `more` follows, carrying `payload`.
fn frame(opcode: u8, more: bool, payload: &[u8]) -> Vec<u8> {
    let fin = if more { 0 } else { 0x80 };
    let mut bytes = vec![fin | opcode, u8::try_from(payload.len()).unwrap()];
    bytes.extend_from_slice(payload);
    bytes
}

/// The next frame the board sent, its opcode and its payload unmasked.
async fn read_frame(socket: &mut TcpStream) -> (u8, Vec<u8>) {
    let opcode = socket.read_u8().await.unwrap() & 0x0f;
    let length = usize::from(socket.read_u8().await.unwrap() & 0x7f);
    let mut mask = [0; 4];
    let _read = socket.read_exact(&mut mask).await.unwrap();
    let mut payload = vec![0; length];
    let _read = socket.read_exact(&mut payload).await.unwrap();
    for (byte, key) in payload.iter_mut().zip(mask.iter().cycle()) {
        *byte ^= key;
    }
    (opcode, payload)
}

#[tokio::test(flavor = "current_thread")]
async fn frames_cross_both_ways_and_a_ping_is_answered() {
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    LocalSet::new()
        .run_until(async {
            let address = scripted(UPGRADED, |mut socket| async move {
                socket.write_all(&frame(0x9, false, b"hi")).await.unwrap();
                socket.write_all(&frame(0x1, true, b"split ")).await.unwrap();
                socket.write_all(&frame(0x0, false, b"in two")).await.unwrap();
                // Binary, though the frame that completes it is a continuation.
                socket.write_all(&frame(0x2, true, &[1])).await.unwrap();
                socket.write_all(&frame(0x0, false, &[2])).await.unwrap();
                socket.write_all(&frame(0xA, false, b"")).await.unwrap();
                let pong = read_frame(&mut socket).await;
                let text = read_frame(&mut socket).await;
                let binary = read_frame(&mut socket).await;
                let close = read_frame(&mut socket).await;
                seen_tx.send([pong, text, binary, close]).unwrap();
            })
            .await;
            let mut transport = transport_to(address, Board::default());
            transport.connect().await.unwrap();
            assert_eq!(
                transport.recv(5000).await,
                Ok(Some(Frame::Text("split in two".to_owned())))
            );
            assert_eq!(transport.recv(5000).await, Ok(Some(Frame::Binary(vec![1, 2]))));
            assert_eq!(transport.recv(10).await, Ok(None));
            transport.send(Frame::Text("up".to_owned())).await.unwrap();
            transport.send(Frame::Binary(vec![3])).await.unwrap();
            transport.close().await;
            let seen = tokio::time::timeout(PATIENCE, seen_rx).await.unwrap().unwrap();
            assert_eq!(
                seen,
                [
                    (0xA, b"hi".to_vec()),
                    (0x1, b"up".to_vec()),
                    (0x2, vec![3]),
                    (0x8, Vec::new())
                ]
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_hub_that_closes_or_refuses_the_upgrade_is_reported() {
    LocalSet::new()
        .run_until(async {
            let closing = scripted(UPGRADED, |mut socket| async move {
                socket.write_all(&frame(0x8, false, b"")).await.unwrap();
            })
            .await;
            let mut transport = transport_to(closing, Board::default());
            transport.connect().await.unwrap();
            assert_eq!(transport.recv(5000).await, Err(NetError::Closed));

            let refusing = scripted("HTTP/1.1 403 Forbidden\r\n\r\n", |_socket| async {}).await;
            let mut transport = transport_to(refusing, Board::default());
            assert_eq!(transport.connect().await, Err(NetError::Handshake));
        })
        .await;
}
