//! Covers the `runner` world calls no shipped guest makes on a board: the websocket used out of order, and the bucket
//! calls a board refuses.
#![cfg(test)]

#[cfg(not(feature = "std"))]
use alloc::{borrow::ToOwned as _, string::String, vec, vec::Vec};
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use edge_toolkit::ws::{ConnectStatus, ServerMessage};
use wasmtime::ResourceLimiter as _;
use wasmtime::component::Resource;

use super::host::{BoardHost, Buckets};
use super::memory::MemoryBudget;
use super::platform::{Console, Platform, Transport};
use crate::bindings::et::ws_messages::messages::ClientMessage;
use crate::bindings::et::ws_wasi::ws::{Host as _, State, WsError};
use crate::bindings::wasi::keyvalue::store::{Error, Host as _, HostBucket};
use crate::bindings::wasi::logging::logging::Level;
use crate::convert::Frame;
use crate::keyvalue::Bucket;

/// A WebAssembly page, the unit a memory grows by.
const PAGE: usize = 64 * 1024;

/// A board with nothing to say: its clock stands still and its log goes nowhere.
struct Quiet;

impl Platform for Quiet {
    fn now(&self) -> u64 {
        0
    }

    async fn sleep_until(&self, _deadline: u64) {}

    fn log(&self, _level: Level, _context: &str, _message: &str) {}

    fn write(&self, _console: Console, _bytes: &[u8]) {}
}

/// A hub that acks every connect, and has nothing else to say.
struct Acking;

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the hub answers at once from memory, where a board's transport awaits its socket"
)]
impl Transport for Acking {
    type Error = &'static str;

    async fn connect(&mut self) -> Result<(), &'static str> {
        Ok(())
    }

    async fn send(&mut self, _frame: Frame) -> Result<(), &'static str> {
        Ok(())
    }

    async fn recv(&mut self, _timeout_ms: u32) -> Result<Option<Frame>, &'static str> {
        let ack = ServerMessage::ConnectAck {
            agent_id: "agent-1".to_owned(),
            status: ConnectStatus::Assigned,
        };
        Ok(Some(Frame::Text(serde_json::to_string(&ack).unwrap())))
    }

    async fn close(&mut self) {}
}

/// `future`'s output, which every call below has at once, since nothing in them waits.
fn now<F>(future: F) -> F::Output
where
    F: Future,
{
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("a board call waited on nothing"),
    }
}

/// A guest's store data on a quiet board with an acking hub.
fn host() -> BoardHost<Quiet, Acking> {
    on_board(Buckets::default(), roomy())
}

/// A guest's store data on a quiet board with an acking hub, sharing `buckets` and `memory` with the board's others.
fn on_board(buckets: Buckets, memory: MemoryBudget) -> BoardHost<Quiet, Acking> {
    BoardHost::new(Quiet, Acking, buckets, memory)
}

/// A budget no guest here spends.
fn roomy() -> MemoryBudget {
    MemoryBudget::new(usize::MAX)
}

/// The bucket `identifier` opens.
fn open(host: &mut BoardHost<Quiet, Acking>, identifier: &str) -> Resource<Bucket> {
    now(host.open(identifier.to_owned())).unwrap()
}

#[test]
fn the_websocket_refuses_messages_until_it_connects() {
    let mut host = host();
    assert!(matches!(now(host.get_state()), State::Closed));
    assert!(matches!(
        now(host.send(ClientMessage::ListAgents)),
        Err(WsError::NotConnected)
    ));
    assert!(matches!(now(host.recv(0)), Err(WsError::NotConnected)));
    now(host.disconnect());
    assert!(matches!(now(host.get_state()), State::Closed));
}

#[test]
fn a_connected_websocket_refuses_a_second_connect() {
    let mut host = host();
    now(host.connect()).unwrap();
    assert!(matches!(now(host.connect()), Err(WsError::AlreadyConnected)));
    assert!(matches!(now(host.get_state()), State::Connected));
    assert_eq!(now(host.agent_id()), "agent-1");
    now(host.disconnect());
    assert!(matches!(now(host.get_state()), State::Closed));
    assert_eq!(now(host.agent_id()), String::new());
}

#[test]
fn a_module_bucket_holds_nothing_and_cannot_be_written() {
    let mut host = host();
    let bucket = open(&mut host, "modules/wasi-math1");
    let rep = bucket.rep();
    assert!(matches!(
        now(host.get(Resource::new_own(rep), "pkg/model.onnx".to_owned())),
        Ok(None)
    ));
    assert!(matches!(
        now(host.set(Resource::new_own(rep), "pkg/model.onnx".to_owned(), vec![1])),
        Err(Error::AccessDenied)
    ));
    now(HostBucket::drop(&mut host, bucket)).unwrap();
}

#[test]
fn guests_handed_the_same_buckets_read_what_each_other_wrote() {
    let buckets = Buckets::default();
    let mut writer = on_board(buckets.clone(), roomy());
    let mut reader = on_board(buckets, roomy());
    let written = open(&mut writer, "agent-1");
    now(writer.set(written, "math1-input.json".to_owned(), vec![7])).unwrap();
    let read = open(&mut reader, "agent-1");
    assert_eq!(
        now(reader.get(read, "math1-input.json".to_owned())).unwrap(),
        Some(vec![7])
    );
    let mut other_board = host();
    let elsewhere = open(&mut other_board, "agent-1");
    assert_eq!(
        now(other_board.get(elsewhere, "math1-input.json".to_owned())).unwrap(),
        None
    );
}

#[test]
fn a_bucket_refuses_the_calls_a_board_does_not_implement() {
    let mut host = host();
    let rep = open(&mut host, "agent-1").rep();
    let refusals = [
        now(host.delete(Resource::new_own(rep), "key".to_owned())).err(),
        now(host.exists(Resource::new_own(rep), "key".to_owned())).err(),
        now(host.list_keys(Resource::new_own(rep), None)).err(),
    ];
    let details: Vec<_> = refusals
        .into_iter()
        .map(|refusal| match refusal {
            Some(Error::Other(detail)) => detail,
            _ => panic!("a bucket call a board does not implement did not refuse as other"),
        })
        .collect();
    assert_eq!(
        details,
        [
            "delete not implemented",
            "exists not implemented",
            "list-keys not implemented"
        ]
    );
}

#[test]
fn guests_grow_only_into_what_the_board_has_left() {
    let memory = MemoryBudget::new(3 * PAGE);
    let mut first = on_board(Buckets::default(), memory.clone());
    let mut second = on_board(Buckets::default(), memory.clone());
    assert!(first.allowance().memory_growing(0, 2 * PAGE, None).unwrap());
    assert!(!second.allowance().memory_growing(0, 2 * PAGE, None).unwrap());
    assert!(second.allowance().memory_growing(0, PAGE, None).unwrap());
    assert_eq!(memory.left(), 0);
    assert!(!first.allowance().memory_growing(2 * PAGE, 3 * PAGE, None).unwrap());
    drop(first);
    assert_eq!(memory.left(), 2 * PAGE);
    drop(second);
    assert_eq!(memory.left(), 3 * PAGE);
}

#[test]
fn a_growth_that_fails_after_all_returns_what_it_drew() {
    let memory = MemoryBudget::new(4 * PAGE);
    let mut guest = on_board(Buckets::default(), memory.clone());
    assert!(guest.allowance().memory_growing(0, PAGE, None).unwrap());
    assert!(
        guest
            .allowance()
            .memory_growing(PAGE, 3 * PAGE, Some(2 * PAGE))
            .unwrap()
    );
    guest
        .allowance()
        .memory_grow_failed(wasmtime::Error::msg("past the maximum"))
        .unwrap();
    assert_eq!(memory.left(), 3 * PAGE);

    let pointer = size_of::<usize>();
    assert!(guest.allowance().table_growing(0, 8, None).unwrap());
    assert_eq!(memory.left(), 3 * PAGE - 8 * pointer);
    guest
        .allowance()
        .table_grow_failed(wasmtime::Error::msg("past the maximum"))
        .unwrap();
    assert_eq!(memory.left(), 3 * PAGE);
    drop(guest);
    assert_eq!(memory.left(), 4 * PAGE);
}
