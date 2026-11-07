//! Runs guests through the board's runner on this host, with a hub played in memory, a clock that jumps to each
//! deadline a guest sleeps until, and a log kept in a vector.
//!
//! A component is compiled ahead of time for the board engine this build has; with `core-modules`, a core module runs
//! on the board's interpreter as it is.
#![cfg(test)]

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use edge_toolkit::board::Guest;
use edge_toolkit::config::get_project_root;
use edge_toolkit::ws::{
    AgentConnectionState, AgentSummary, ClientMessage, ConnectStatus, MessageDeliveryStatus, MessageScope,
    ServerMessage,
};
#[cfg(feature = "core-modules")]
use et_ws_test_server::math1::{MATH1_INPUT_FILENAME, MATH1_INPUT_JSON, MATH1_OUTPUT_FILENAME, verify_math1_model};
use et_ws_wasi_runner::bindings::et::ws_wasi::ws::WsError;
use et_ws_wasi_runner::bindings::exports::et::ws_wasi::entry::EntryError;
use et_ws_wasi_runner::bindings::wasi::logging::logging::Level;
#[cfg(feature = "core-modules")]
use et_ws_wasi_runner::board::run_core_module;
use et_ws_wasi_runner::board::{
    BoardError, Buckets, Console, MemoryBudget, Platform, RERUN_DELAY, Transport, run, run_guest,
};
use et_ws_wasi_runner::convert::Frame;
#[cfg(feature = "core-modules")]
use et_ws_wasi_runner::core_module::CoreError;
use et_ws_wasi_runner::engine::{NATIVE_PULLEY, board_engine, precompile};
use rstest::rstest;

/// The id the hub assigns the guest.
const AGENT_ID: &str = "board-agent";
/// One WebAssembly page.
#[cfg(feature = "core-modules")]
const PAGE: usize = 64 * 1024;
/// Two pages.
#[cfg(feature = "core-modules")]
const TWO_PAGES: usize = 2 * PAGE;

/// A core module calling every `et` import that reaches the hub or its storage, returning which step went wrong.
///
/// It sends before it connects, which is refused, then connects, sends `et-list-agents` and reads the ack and the list.
/// It stores `value` under `key` in its own bucket and reads it back, finds nothing in a module's bucket and is refused
/// a write there, logs `logged`, and disconnects.
#[cfg(feature = "core-modules")]
const REACHES_THE_HUB: &str = r#"(module
    (import "et" "connect" (func $connect (result i32)))
    (import "et" "send" (func $send (param i32 i32) (result i32)))
    (import "et" "recv" (func $recv (param i32) (result i32)))
    (import "et" "kv_get" (func $kv_get (param i32 i32 i32 i32) (result i32)))
    (import "et" "kv_set" (func $kv_set (param i32 i32 i32 i32 i32 i32) (result i32)))
    (import "et" "log" (func $log (param i32 i32 i32 i32 i32)))
    (import "et" "disconnect" (func $disconnect))
    (memory 1)
    (data (i32.const 0) "{\"type\":\"et-list-agents\"}")
    (data (i32.const 32) "board-agent")
    (data (i32.const 48) "modules/board")
    (data (i32.const 64) "key")
    (data (i32.const 72) "value")
    (data (i32.const 80) "logged")
    (func (export "run") (result i32)
        (if (i32.ne (call $send (i32.const 0) (i32.const 25)) (i32.const -3)) (then (return (i32.const 1))))
        (if (i32.ne (call $connect) (i32.const 0)) (then (return (i32.const 2))))
        (if (i32.ne (call $send (i32.const 0) (i32.const 25)) (i32.const 0)) (then (return (i32.const 3))))
        (if (i32.le_s (call $recv (i32.const 0)) (i32.const 0)) (then (return (i32.const 4))))
        (if (i32.le_s (call $recv (i32.const 0)) (i32.const 0)) (then (return (i32.const 5))))
        (if (i32.ne
            (call $kv_set (i32.const 32) (i32.const 11) (i32.const 64) (i32.const 3) (i32.const 72) (i32.const 5))
            (i32.const 0)) (then (return (i32.const 6))))
        (if (i32.ne (call $kv_get (i32.const 32) (i32.const 11) (i32.const 64) (i32.const 3)) (i32.const 1))
            (then (return (i32.const 7))))
        (if (i32.ne (call $kv_get (i32.const 48) (i32.const 13) (i32.const 64) (i32.const 3)) (i32.const 0))
            (then (return (i32.const 8))))
        (if (i32.ne
            (call $kv_set (i32.const 48) (i32.const 13) (i32.const 64) (i32.const 3) (i32.const 72) (i32.const 5))
            (i32.const -5)) (then (return (i32.const 9))))
        (call $log (i32.const 2) (i32.const 32) (i32.const 11) (i32.const 80) (i32.const 6))
        (call $disconnect)
        (i32.const 0)))"#;

/// A core module that grows its one page of memory by another, returning 0 when the grow fails and 2 when it does not.
#[cfg(feature = "core-modules")]
const GROWS_A_PAGE: &str = r#"(module
    (memory 1)
    (func (export "run") (result i32) (i32.add (memory.grow (i32.const 1)) (i32.const 1))))"#;

/// A core module that connects and disconnects, returning 1 when the hub refuses it.
#[cfg(feature = "core-modules")]
const CONNECTS: &str = r#"(module
    (import "et" "connect" (func $connect (result i32)))
    (import "et" "disconnect" (func $disconnect))
    (func (export "run") (result i32)
        (if (i32.ne (call $connect) (i32.const 0)) (then (return (i32.const 1))))
        (call $disconnect)
        (i32.const 0)))"#;

/// A board's clock and log, played by this host.
///
/// The clock only moves when the guest sleeps, and the log keeps every line logged or written, in order.
#[derive(Clone, Default)]
struct Host {
    clock: Arc<AtomicU64>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Platform for Host {
    fn now(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }

    async fn sleep_until(&self, deadline: u64) {
        let _before = self.clock.fetch_max(deadline, Ordering::SeqCst);
    }

    fn log(&self, _level: Level, _context: &str, message: &str) {
        self.lines.lock().unwrap().push(message.to_owned());
    }

    fn write(&self, _console: Console, bytes: &[u8]) {
        self.lines
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(bytes).into_owned());
    }
}

/// Where the hub's websocket fails, if it does.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Fault {
    /// Nowhere.
    #[default]
    None,
    /// It cannot be opened.
    Refused,
    /// Every frame sent fails, the connect's included.
    SendFails,
    /// Every frame sent after the connect fails.
    SendFailsAfterConnect,
    /// Every read fails.
    Unreadable,
    /// Every read fails once the frames the hub queued have all been read.
    RecvFailsWhenIdle,
    /// The hub never acknowledges the connect.
    NoAck,
}

/// A hub that answers a connect with an id after the frames it was given.
///
/// It records every frame it is sent, and whether it was closed.
#[derive(Default)]
struct Hub {
    sent: Arc<Mutex<Vec<Frame>>>,
    closed: Arc<Mutex<bool>>,
    replies: VecDeque<Frame>,
    fault: Fault,
    storage: Buckets,
}

impl Hub {
    /// A hub that fails at `fault`, and sends `frames` ahead of its connect ack.
    fn new(fault: Fault, frames: impl IntoIterator<Item = Frame>) -> Self {
        Self {
            replies: frames.into_iter().collect(),
            fault,
            ..Self::default()
        }
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the hub answers at once from memory, where a board's transport awaits its socket"
)]
impl Transport for Hub {
    type Error = &'static str;

    async fn connect(&mut self) -> Result<(), &'static str> {
        if self.fault == Fault::Refused {
            return Err("refused");
        }
        Ok(())
    }

    async fn send(&mut self, frame: Frame) -> Result<(), &'static str> {
        let first = self.sent.lock().unwrap().is_empty();
        if self.fault == Fault::SendFails || (self.fault == Fault::SendFailsAfterConnect && !first) {
            return Err("broken pipe");
        }
        if let Frame::Text(text) = &frame {
            let reply = match serde_json::from_str(text) {
                Ok(ClientMessage::Connect { .. }) if self.fault != Fault::NoAck => Some(ServerMessage::ConnectAck {
                    agent_id: AGENT_ID.to_owned(),
                    status: ConnectStatus::Assigned,
                }),
                Ok(ClientMessage::ListAgents) => Some(ServerMessage::ListAgentsResponse {
                    agents: vec![AgentSummary::new(
                        AGENT_ID.to_owned(),
                        AgentConnectionState::Connected,
                        IpAddr::V4(Ipv4Addr::LOCALHOST),
                    )],
                }),
                _ => None,
            };
            if let Some(reply) = reply {
                self.replies.push_back(text_frame(&reply));
            }
        }
        self.sent.lock().unwrap().push(frame);
        Ok(())
    }

    async fn recv(&mut self, _timeout_ms: u32) -> Result<Option<Frame>, &'static str> {
        match (self.fault, self.replies.pop_front()) {
            (Fault::Unreadable, _) | (Fault::RecvFailsWhenIdle, None) => Err("connection reset"),
            (_, reply) => Ok(reply),
        }
    }

    async fn close(&mut self) {
        *self.closed.lock().unwrap() = true;
    }

    async fn load(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, &'static str> {
        Ok(self.storage.get(bucket, key).ok())
    }

    async fn store(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), &'static str> {
        self.storage.set(bucket, key, value);
        Ok(())
    }
}

/// `message` as the text frame the hub sends it in.
fn text_frame(message: &ServerMessage) -> Frame {
    Frame::Text(serde_json::to_string(message).unwrap())
}

/// `module`'s component as its build left it.
fn component(module: &str) -> Vec<u8> {
    let file = format!("et_ws_{}.wasm", module.replace('-', "_"));
    fs_err::read(
        get_project_root()
            .join("services/ws-modules")
            .join(module)
            .join("pkg")
            .join(file),
    )
    .unwrap()
}

/// `module`'s component as its build left it, compiled for the board engine this build has.
fn compiled(module: &str) -> Vec<u8> {
    precompile(NATIVE_PULLEY, &component(module)).unwrap()
}

/// What running `component` against `hub` left.
///
/// That is how the run ended, every line it logged, every frame it sent, and whether it closed the websocket.
async fn ran_on(component: &[u8], hub: Hub) -> (Result<(), BoardError>, Vec<String>, Vec<Frame>, bool) {
    ran_within(component, hub, MemoryBudget::new(usize::MAX)).await
}

/// What running `component` against `hub` left, its memories and tables drawn from `memory`.
async fn ran_within(
    component: &[u8],
    hub: Hub,
    memory: MemoryBudget,
) -> (Result<(), BoardError>, Vec<String>, Vec<Frame>, bool) {
    ran_by(hub, async |host, hub| {
        run(&board_engine().unwrap(), component, host, hub, memory).await
    })
    .await
}

/// What `run_guest` left, given the board's clock and log and `hub`.
///
/// That is how the run ended, every line logged, every frame sent, and whether the websocket was closed.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "shared with the core module cases when the crate has them"
    )
)]
async fn ran_by<R, F>(hub: Hub, run_guest: R) -> (Result<(), BoardError>, Vec<String>, Vec<Frame>, bool)
where
    R: FnOnce(Host, Hub) -> F,
    F: Future<Output = Result<(), BoardError>>,
{
    let host = Host::default();
    let lines = Arc::clone(&host.lines);
    let (sent, closed) = (Arc::clone(&hub.sent), Arc::clone(&hub.closed));
    let outcome = run_guest(host, hub).await;
    let lines = lines.lock().unwrap().clone();
    let sent = sent.lock().unwrap().clone();
    let closed = *closed.lock().unwrap();
    (outcome, lines, sent, closed)
}

/// What running `module` to completion against a working hub left: every line it logged, and every message it sent.
async fn ran(module: &str) -> (Vec<String>, Vec<ClientMessage>) {
    let (outcome, lines, sent, closed) = ran_on(&compiled(module), Hub::default()).await;
    outcome.unwrap();
    assert!(closed);
    let sent = sent
        .iter()
        .map(|frame| {
            let Frame::Text(text) = frame else {
                panic!("{module} sent a frame other than text: {frame:?}");
            };
            serde_json::from_str(text).unwrap()
        })
        .collect();
    assert!(lines.iter().any(|line| line.contains(&format!("agent_id={AGENT_ID}"))));
    (lines, sent)
}

/// The error `module`'s `entry.run` returned against `hub`.
async fn failed(module: &str, hub: Hub) -> EntryError {
    failed_closing(module, hub).await.0
}

/// The error `module`'s `entry.run` returned against `hub`, and whether the websocket was closed.
async fn failed_closing(module: &str, hub: Hub) -> (EntryError, bool) {
    let (outcome, .., closed) = ran_on(&compiled(module), hub).await;
    let Err(BoardError::Guest(error)) = outcome else {
        panic!("{module} did not fail: {outcome:?}");
    };
    (error, closed)
}

#[tokio::test]
async fn data1_round_trips_a_value_through_the_boards_memory() {
    let (lines, sent) = ran("wasi-data1").await;
    assert!(lines.iter().any(|line| line.contains("VERIFICATION SUCCESS")));
    assert!(matches!(sent.as_slice(), [ClientMessage::Connect { agent_id: None }]));
}

#[tokio::test]
async fn comm1_finds_itself_in_the_roster_and_broadcasts() {
    let (lines, sent) = ran("wasi-comm1").await;
    assert!(lines.iter().any(|line| line.contains("self present in roster")));
    assert!(matches!(
        sent.as_slice(),
        [
            ClientMessage::Connect { agent_id: None },
            ClientMessage::ListAgents,
            ClientMessage::BroadcastMessage { .. }
        ]
    ));
}

#[tokio::test]
async fn math1_sender_broadcasts_its_pointer_once_a_second_of_the_boards_clock() {
    let (outcome, lines, sent, closed) = ran_on(&compiled("wasi-math1-sender"), Hub::default()).await;
    outcome.unwrap();
    assert!(closed && lines.iter().any(|line| line.contains("workflow complete")));
    let [Frame::Text(connect), pointers @ ..] = sent.as_slice() else {
        panic!("math1-sender did not connect first: {sent:?}");
    };
    assert!(matches!(
        serde_json::from_str(connect).unwrap(),
        ClientMessage::Connect { agent_id: None }
    ));
    assert_eq!(pointers.len(), 60);
    assert!(pointers.iter().all(|pointer| matches!(
        pointer,
        Frame::Text(content) if content.contains(r#""bucket":"board-agent""#)
    )));
}

#[tokio::test]
async fn math1_reads_every_message_kind_before_its_pointer_and_finds_no_input_in_a_module_bucket() {
    let frames = [
        ServerMessage::AgentMessage {
            message_id: "message-1".to_owned(),
            from_agent_id: "agent-2".to_owned(),
            scope: MessageScope::Direct,
            server_received_at: "2026-11-02T06:30:00Z".to_owned(),
            message: serde_json::json!({ "n": 2_i32 }),
        },
        ServerMessage::MessageStatus {
            message_id: None,
            status: MessageDeliveryStatus::Queued,
            detail: "queued".to_owned(),
        },
        ServerMessage::Invalid {
            message_id: None,
            detail: "unknown type".to_owned(),
        },
        ServerMessage::Response {
            message: "pong".to_owned(),
        },
    ]
    .into_iter()
    .map(|message| text_frame(&message))
    .chain([
        Frame::Binary(vec![7]),
        Frame::Text(r#"{"type":"math1-input","bucket":"modules/wasi-math1","filename":"math1-input.json"}"#.to_owned()),
    ]);
    let error = failed("wasi-math1", Hub::new(Fault::None, frames)).await;
    assert!(matches!(error, EntryError::Runtime(detail) if detail == "input math1-input.json not found"));
}

#[rstest]
#[case::nothing_arrives(Fault::None)]
#[case::reads_fail(Fault::RecvFailsWhenIdle)]
#[tokio::test]
async fn math1_gives_up_when_no_pointer_arrives(#[case] fault: Fault) {
    let error = failed("wasi-math1", Hub::new(fault, [])).await;
    assert!(matches!(error, EntryError::Runtime(detail) if detail == "did not receive the math1-input pointer"));
}

#[rstest]
#[case::refused(Fault::Refused, "refused", false)]
#[case::send_fails(Fault::SendFails, "broken pipe", true)]
#[case::unreadable(Fault::Unreadable, "connection reset", true)]
#[case::no_ack(Fault::NoAck, "the hub sent no et-connect-ack", true)]
#[tokio::test]
async fn data1_fails_to_connect_through_a_broken_hub(
    #[case] fault: Fault,
    #[case] expected: &str,
    #[case] was_open: bool,
) {
    let (error, closed) = failed_closing("wasi-data1", Hub::new(fault, [])).await;
    assert!(matches!(error, EntryError::Ws(WsError::Transport(reason)) if reason == expected));
    assert_eq!(closed, was_open);
}

#[tokio::test]
async fn data1_fails_to_connect_when_a_frame_claims_a_type_it_does_not_match() {
    let malformed = Frame::Text(r#"{"type":"et-response"}"#.to_owned());
    let (error, closed) = failed_closing("wasi-data1", Hub::new(Fault::None, [malformed])).await;
    assert!(matches!(error, EntryError::Ws(WsError::Decode(_))));
    assert!(closed);
}

#[tokio::test]
async fn comm1_fails_when_its_roster_request_cannot_be_sent() {
    let error = failed("wasi-comm1", Hub::new(Fault::SendFailsAfterConnect, [])).await;
    assert!(matches!(error, EntryError::Ws(WsError::Transport(reason)) if reason == "broken pipe"));
}

/// Every guest the tests above run on the board imports only what the hosted list names.
///
/// So a deployment that checks a guest against the list refuses none the board can run.
#[rstest]
#[case::comm1("wasi-comm1")]
#[case::data1("wasi-data1")]
#[case::math1("wasi-math1")]
#[case::math1_sender("wasi-math1-sender")]
fn a_guest_the_board_runs_imports_only_what_it_hosts(#[case] module: &str) {
    let guest = Guest::read(&component(module)).unwrap();
    assert_eq!(guest.unhosted().count(), 0);
    assert!(guest.memory > 0);
}

#[test]
fn a_guest_that_needs_a_gpu_or_a_model_imports_what_the_board_does_not_host() {
    let guest = Guest::read(&component("wasi-graphics-info")).unwrap();
    let unhosted = guest.unhosted().collect::<Vec<_>>();
    assert!(unhosted.iter().any(|import| import.starts_with("wasi:webgpu/")));
    assert!(unhosted.iter().any(|import| import.starts_with("wasi:nn/")));
}

#[tokio::test]
async fn a_guest_runs_within_the_board_s_memory_and_returns_it_when_it_ends() {
    let module = "wasi-data1";
    let starts_with = usize::try_from(Guest::read(&component(module)).unwrap().memory).unwrap();
    let roomy = MemoryBudget::new(starts_with.saturating_mul(4));
    let (outcome, ..) = ran_within(&compiled(module), Hub::default(), roomy.clone()).await;
    outcome.unwrap();
    assert_eq!(roomy.left(), starts_with.saturating_mul(4));

    let component = compiled(module);
    let outcome = ran_short(starts_with, |short| ran_within(&component, Hub::default(), short)).await;
    assert!(matches!(outcome, Err(BoardError::Wasmtime(_))));
}

/// How a guest starting with `starts_with` bytes of memories ended, run by `run_within` on one byte fewer.
///
/// It must not have run at all, logging and sending nothing, and must have drawn nothing.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "shared with the core module cases when the crate has them"
    )
)]
async fn ran_short<R, F>(starts_with: usize, run_within: R) -> Result<(), BoardError>
where
    R: FnOnce(MemoryBudget) -> F,
    F: Future<Output = (Result<(), BoardError>, Vec<String>, Vec<Frame>, bool)>,
{
    let short = MemoryBudget::new(starts_with.saturating_sub(1));
    let (outcome, lines, sent, _) = run_within(short.clone()).await;
    assert!(lines.is_empty() && sent.is_empty());
    assert_eq!(short.left(), starts_with.saturating_sub(1));
    outcome
}

#[tokio::test]
async fn bytes_that_were_not_compiled_for_the_board_do_not_load() {
    let (outcome, lines, sent, _) = ran_on(b"not a component", Hub::default()).await;
    assert!(matches!(outcome, Err(BoardError::Wasmtime(_))));
    assert!(lines.is_empty() && sent.is_empty());
}

/// What `run_guest` reported of each run of `guest`, each run's transport the next of `hubs`, and the clock after.
#[cfg_attr(
    feature = "core-modules",
    expect(
        clippy::future_not_send,
        reason = "SpaceWasm's engine is not Send, and each case here awaits it on a runtime of its own"
    )
)]
async fn supervised(guest: &[u8], hubs: Vec<Hub>) -> (Vec<String>, u64) {
    let host = Host::default();
    let (lines, clock) = (Arc::clone(&host.lines), Arc::clone(&host.clock));
    let mut hubs = hubs.into_iter();
    run_guest(
        "guest",
        guest,
        host,
        move || hubs.next().unwrap(),
        MemoryBudget::new(usize::MAX),
    )
    .await;
    let reports = lines
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.starts_with("runner guest: "))
        .cloned()
        .collect();
    (reports, clock.load(Ordering::SeqCst))
}

/// Check that `reports` say the guest failed in itself, then finished once run again [`RERUN_DELAY`] later on `clock`.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "shared with the core module case when the crate has them"
    )
)]
fn assert_reran_once(reports: &[String], clock: u64) {
    assert!(matches!(
        reports,
        [failed, finished] if failed.ends_with("; running it again\n") && finished == "runner guest: finished\n"
    ));
    assert_eq!(clock, RERUN_DELAY);
}

/// Check that `reports` say the guest's one run ended in a failure it was not run again for, the clock never moving.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "shared with the core module case when the crate has them"
    )
)]
fn assert_not_rerun(reports: &[String], clock: u64) {
    assert!(matches!(reports, [ended] if !ended.ends_with("; running it again\n") && !ended.ends_with("finished\n")));
    assert_eq!(clock, 0);
}

#[tokio::test]
async fn a_component_failing_in_itself_is_run_again_on_a_fresh_transport_until_it_finishes() {
    let (reports, clock) = supervised(
        &compiled("wasi-data1"),
        vec![Hub::new(Fault::Refused, []), Hub::default()],
    )
    .await;
    assert_reran_once(&reports, clock);
}

#[tokio::test]
async fn a_component_that_does_not_load_is_not_run_again() {
    let (reports, clock) = supervised(b"not a component", vec![Hub::default()]).await;
    assert_not_rerun(&reports, clock);
}

/// What running the core module `module` against `hub` left, its memory drawn from `memory`.
#[cfg(feature = "core-modules")]
#[expect(
    clippy::future_not_send,
    reason = "SpaceWasm's engine is not Send, and each case here awaits it on a runtime of its own"
)]
async fn ran_core_module(
    module: &[u8],
    hub: Hub,
    memory: MemoryBudget,
) -> (Result<(), BoardError>, Vec<String>, Vec<Frame>, bool) {
    ran_by(hub, async |host, hub| run_core_module(module, host, hub, memory).await).await
}

#[cfg(feature = "core-modules")]
#[tokio::test]
async fn core_math1_stores_the_model_fitted_to_the_input_its_pointer_names() {
    let pointer = format!(r#"{{"type":"math1-input","bucket":"sender","filename":"{MATH1_INPUT_FILENAME}"}}"#);
    let hub = Hub::new(Fault::None, [Frame::Text(pointer)]);
    let storage = hub.storage.clone();
    storage.set("sender", MATH1_INPUT_FILENAME, MATH1_INPUT_JSON.as_bytes().to_vec());
    let module = component("core-math1");
    let (outcome, lines, _, closed) = ran_core_module(&module, hub, MemoryBudget::new(usize::MAX)).await;
    outcome.unwrap();
    assert!(closed && lines.iter().any(|line| line == "workflow complete"));
    let stored = storage.get(AGENT_ID, MATH1_OUTPUT_FILENAME).unwrap();
    let model: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    let weight = model.get("weight").and_then(serde_json::Value::as_f64).unwrap();
    let bias = model.get("bias").and_then(serde_json::Value::as_f64).unwrap();
    verify_math1_model(weight, bias).unwrap();
}

#[cfg(feature = "core-modules")]
#[tokio::test]
async fn a_core_module_reaches_the_hub_and_its_storage_through_the_board() {
    let hub = Hub::default();
    let storage = hub.storage.clone();
    let module = wat::parse_str(REACHES_THE_HUB).unwrap();
    let (outcome, lines, sent, closed) = ran_core_module(&module, hub, MemoryBudget::new(usize::MAX)).await;
    outcome.unwrap();
    assert!(closed && lines.iter().any(|line| line == "logged"));
    assert_eq!(storage.get(AGENT_ID, "key").unwrap(), b"value");
    let [Frame::Text(connect), Frame::Text(list)] = sent.as_slice() else {
        panic!("the guest did not send a connect and one message: {sent:?}");
    };
    assert!(connect.contains("et-connect") && list.contains("et-list-agents"));
}

#[cfg(feature = "core-modules")]
#[tokio::test]
async fn a_core_module_that_starts_with_more_memory_than_is_left_does_not_load() {
    let module = component("core-math1");
    let starts_with = usize::try_from(Guest::read(&module).unwrap().memory).unwrap();
    let outcome = ran_short(starts_with, |short| ran_core_module(&module, Hub::default(), short)).await;
    assert!(matches!(outcome, Err(BoardError::CoreModule(CoreError::Decode(_)))));
}

#[cfg(feature = "core-modules")]
#[tokio::test]
async fn a_core_module_grows_only_as_far_as_the_board_s_memory_and_returns_it_when_it_ends() {
    let module = wat::parse_str(GROWS_A_PAGE).unwrap();
    let one_page = MemoryBudget::new(PAGE);
    let (outcome, ..) = ran_core_module(&module, Hub::default(), one_page.clone()).await;
    outcome.unwrap();
    assert_eq!(one_page.left(), PAGE);

    let two_pages = MemoryBudget::new(TWO_PAGES);
    let (outcome, ..) = ran_core_module(&module, Hub::default(), two_pages.clone()).await;
    assert!(matches!(outcome, Err(BoardError::CoreModule(CoreError::Guest(2_i32)))));
    assert_eq!(two_pages.left(), TWO_PAGES);
}

#[cfg(feature = "core-modules")]
#[tokio::test]
async fn a_core_module_failing_in_itself_is_run_again_on_a_fresh_transport_until_it_finishes() {
    let module = wat::parse_str(CONNECTS).unwrap();
    let (reports, clock) = supervised(&module, vec![Hub::new(Fault::Refused, []), Hub::default()]).await;
    assert_reran_once(&reports, clock);
}

#[cfg(feature = "core-modules")]
#[tokio::test]
async fn a_core_module_with_no_run_is_not_run_again() {
    let (reports, clock) = supervised(&wat::parse_str("(module)").unwrap(), vec![Hub::default()]).await;
    assert_not_rerun(&reports, clock);
}
