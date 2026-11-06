//! Covers running a Wasm 1.0 core module on `SpaceWasm`: what a module must export, each `et` import and every answer
//! it gives the guest, the traps a bad call raises, and stopping a run that is dropped. Guests are inline WAT, run
//! against a scripted host; the last case runs one against a real hub through the runner's own `HostState`.
#![cfg(test)]

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_core_guest::abi;
use et_ws_wasi_runner::HostState;
use et_ws_wasi_runner::bindings::et::ws_wasi::ws::WsError;
use et_ws_wasi_runner::bindings::wasi::keyvalue::store::Error as StoreError;
use et_ws_wasi_runner::bindings::wasi::logging::logging::Level;
use et_ws_wasi_runner::core_module::{CoreError, CoreHost, is_core_module, run};
use et_ws_wasi_runner::host::run_core_module;
use spacewasm::TrapReason;
use tokio::sync::oneshot::{self, error::RecvError};
use tokio::time::error::Elapsed;

/// Every `et` import, declared as a guest declares them.
const IMPORTS: &str = r#"
    (import "et" "connect" (func $connect (result i32)))
    (import "et" "agent_id" (func $agent_id (result i32)))
    (import "et" "send" (func $send (param i32 i32) (result i32)))
    (import "et" "recv" (func $recv (param i32) (result i32)))
    (import "et" "take" (func $take (param i32 i32) (result i32)))
    (import "et" "disconnect" (func $disconnect))
    (import "et" "kv_get" (func $kv_get (param i32 i32 i32 i32) (result i32)))
    (import "et" "kv_set" (func $kv_set (param i32 i32 i32 i32 i32 i32) (result i32)))
    (import "et" "log" (func $log (param i32 i32 i32 i32 i32)))
    (memory (export "memory") 1)
"#;

/// The `et-list-agents` message as WAT data, 25 bytes once parsed.
const LIST_AGENTS: &str = r#"{\"type\":\"et-list-agents\"}"#;

/// Sending the 25 bytes of [`LIST_AGENTS`] placed at address 0.
const SEND_LIST_AGENTS: &str = "(call $send (i32.const 0) (i32.const 25))";

/// Whether a run ended with the error a case expects.
type Expected = fn(&CoreError) -> bool;

/// The host as a script: what it answers each call with, and a record of what the guest did.
#[derive(Default)]
struct Script {
    /// Whether the websocket is open.
    connected: bool,
    /// What `send` fails with, when set.
    send_error: Option<WsError>,
    /// What each `recv` answers, in order; a timeout once they run out.
    inbox: VecDeque<Result<Option<ServerMessage>, WsError>>,
    /// Whether every storage request fails.
    store_fails: bool,
    /// The storage buckets: `(bucket, key)` to value.
    stored: BTreeMap<(String, String), Vec<u8>>,
    /// Every message sent.
    sent: Vec<ClientMessage>,
    /// Every log entry: level, context, message.
    logged: Vec<(Level, String, String)>,
    /// Whether the guest disconnected.
    disconnected: bool,
}

impl Script {
    /// The value the test expects stored under `key` in `bucket`.
    fn value(&self, bucket: &str, key: &str) -> &[u8] {
        &self.stored[&(bucket.to_owned(), key.to_owned())]
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "CoreHost's methods return futures, which a script answers at once"
)]
impl CoreHost for &mut Script {
    /// Open the websocket, which a second open finds already open.
    async fn connect(&mut self) -> Result<(), WsError> {
        if self.connected {
            return Err(WsError::AlreadyConnected);
        }
        self.connected = true;
        Ok(())
    }

    async fn agent_id(&mut self) -> String {
        "agent-1".to_owned()
    }

    /// Record `message`, unless the script fails the send.
    async fn send(&mut self, message: ClientMessage) -> Result<(), WsError> {
        if let Some(error) = self.send_error.take() {
            return Err(error);
        }
        self.sent.push(message);
        Ok(())
    }

    /// The inbox's next answer, or a timeout once it is empty.
    async fn recv(&mut self, _timeout_ms: u32) -> Result<Option<ServerMessage>, WsError> {
        let Some(answer) = self.inbox.pop_front() else {
            return Ok(None);
        };
        answer
    }

    async fn disconnect(&mut self) {
        self.disconnected = true;
    }

    /// The stored value, unless the script fails storage.
    async fn get(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        if self.store_fails {
            return Err(StoreError::AccessDenied);
        }
        Ok(self.stored.get(&(bucket.to_owned(), key.to_owned())).cloned())
    }

    /// Store the value, unless the script fails storage.
    async fn set(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        if self.store_fails {
            return Err(StoreError::AccessDenied);
        }
        let _replaced = self.stored.insert((bucket.to_owned(), key.to_owned()), value);
        Ok(())
    }

    async fn log(&mut self, level: Level, context: &str, message: &str) {
        self.logged.push((level, context.to_owned(), message.to_owned()));
    }
}

/// A host a run owns, whose `recv` never answers, and whose dropping the test sees.
struct Held {
    /// Never sent on, so its receiver fails exactly when the host is dropped.
    _released: oneshot::Sender<()>,
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "CoreHost's methods return futures, which this host answers at once, all but recv"
)]
impl CoreHost for Held {
    async fn connect(&mut self) -> Result<(), WsError> {
        Ok(())
    }

    async fn agent_id(&mut self) -> String {
        String::default()
    }

    async fn send(&mut self, _message: ClientMessage) -> Result<(), WsError> {
        Ok(())
    }

    /// Wait for a message that never comes.
    async fn recv(&mut self, _timeout_ms: u32) -> Result<Option<ServerMessage>, WsError> {
        std::future::pending().await
    }

    async fn disconnect(&mut self) {}

    async fn get(&mut self, _bucket: &str, _key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(None)
    }

    async fn set(&mut self, _bucket: &str, _key: &str, _value: Vec<u8>) -> Result<(), StoreError> {
        Ok(())
    }

    async fn log(&mut self, _level: Level, _context: &str, _message: &str) {}
}

/// A core module with every `et` import and one page of memory, plus `body`.
fn module(body: &str) -> Vec<u8> {
    wat::parse_str(format!("(module {IMPORTS} {body})")).unwrap()
}

/// A core module whose `run` is `instructions`, with `data` placed at address 0.
fn guest(data: &str, instructions: &str) -> Vec<u8> {
    module(&format!(
        r#"(data (i32.const 0) "{data}") (func (export "run") (result i32) {instructions})"#
    ))
}

/// Run `wasm` against `script`.
#[expect(
    clippy::future_not_send,
    reason = "SpaceWasm's engine is neither Send nor Sync, and each test awaits it on its own runtime"
)]
async fn run_on(wasm: &[u8], script: &mut Script) -> Result<(), CoreError> {
    run(wasm, script).await
}

#[tokio::test]
async fn run_returning_success_succeeds() {
    run_on(&guest("", "(i32.const 0)"), &mut Script::default())
        .await
        .unwrap();
}

#[test]
fn a_core_module_is_told_by_its_header_from_a_component_and_from_compiled_code() {
    assert!(is_core_module(&guest("", "(i32.const 0)")));
    assert!(!is_core_module(&wat::parse_str("(component)").unwrap()));
    assert!(!is_core_module(b"\x7fELF"));
}

#[tokio::test]
async fn run_returning_a_failure_is_the_guests_failure() {
    let ended = run_on(&guest("", "(i32.const 7)"), &mut Script::default()).await;
    assert!(matches!(ended, Err(CoreError::Guest(7_i32))));
}

#[tokio::test]
async fn a_module_needs_a_run_function_of_its_own() {
    let unnamed = module(r#"(func (export "main") (result i32) (i32.const 0))"#);
    let memory = module(r#"(export "run" (memory 0))"#);
    let imported = module(r#"(export "run" (func $connect))"#);
    for wasm in [unnamed, memory, imported] {
        assert!(matches!(
            run_on(&wasm, &mut Script::default()).await,
            Err(CoreError::NoRun)
        ));
    }
}

#[tokio::test]
async fn run_must_take_nothing_and_return_an_i32() {
    let taking = module(r#"(func (export "run") (param i32) (result i32) (i32.const 0))"#);
    let returning_nothing = module(r#"(func (export "run"))"#);
    for wasm in [taking, returning_nothing] {
        let ended = run_on(&wasm, &mut Script::default()).await;
        assert!(matches!(ended, Err(CoreError::RunSignature)));
    }
}

#[tokio::test]
async fn the_start_function_runs_before_run() {
    let wasm = module(
        r#"(func $start (drop (call $connect))) (start $start)
        (func (export "run") (result i32) (call $connect))"#,
    );
    let mut script = Script::default();
    let ended = run_on(&wasm, &mut script).await;
    assert!(matches!(ended, Err(CoreError::Guest(abi::ALREADY_CONNECTED))));
    let (level, context, message) = script.logged.first().unwrap();
    assert_eq!(*level, Level::Warn);
    assert_eq!(context, "et-ws-wasi-runner");
    assert!(message.starts_with("connect failed"));
}

#[tokio::test]
async fn a_module_that_cannot_run_to_its_end_fails() {
    let trapping_start = module(
        r#"(func $start unreachable) (start $start)
        (func (export "run") (result i32) (i32.const 0))"#,
    );
    // Bulk memory is a proposal past Wasm 1.0.
    let bulk = guest(
        "",
        "(memory.copy (i32.const 0) (i32.const 8) (i32.const 8)) (i32.const 0)",
    );
    // Ahead of `IMPORTS`, since WAT takes no import after a memory.
    let missing = r#"(import "et" "missing" (func))"#;
    let unlinked = wat::parse_str(format!(
        r#"(module {missing} {IMPORTS} (func (export "run") (result i32) (i32.const 0)))"#
    ))
    .unwrap();
    let cases: [(Vec<u8>, Expected); 4] = [
        (trapping_start, |error| {
            matches!(error, CoreError::Trap(TrapReason::Unreachable))
        }),
        (guest("", "unreachable"), |error| {
            matches!(error, CoreError::Trap(TrapReason::Unreachable))
        }),
        (bulk, |error| matches!(error, CoreError::Decode(_))),
        (unlinked, |error| matches!(error, CoreError::Decode(_))),
    ];
    for (wasm, expected) in cases {
        let error = run_on(&wasm, &mut Script::default()).await.unwrap_err();
        assert!(expected(&error));
    }
}

#[tokio::test]
async fn a_long_computation_runs_to_its_end() {
    let counting = guest(
        "",
        "(local $left i32) (local.set $left (i32.const 1000000))
        (loop $more (local.set $left (i32.sub (local.get $left) (i32.const 1))) (br_if $more (local.get $left)))
        (local.get $left)",
    );
    run_on(&counting, &mut Script::default()).await.unwrap();
}

#[tokio::test]
async fn the_agent_id_is_taken_out_of_the_slot() {
    // The id goes into memory at 100 and becomes the bucket the guest stores "v" under "k" in.
    let wasm = guest(
        "kv",
        "(local $len i32) (local.set $len (call $agent_id))
        (if (i32.ne (call $take (i32.const 100) (i32.const 0)) (local.get $len)) (then (return (i32.const 10))))
        (if (i32.ne (call $take (i32.const 100) (local.get $len)) (local.get $len)) (then (return (i32.const 11))))
        (call $kv_set (i32.const 100) (local.get $len) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))",
    );
    let mut script = Script::default();
    run_on(&wasm, &mut script).await.unwrap();
    assert_eq!(script.value("agent-1", "k"), b"v");
}

#[tokio::test]
async fn send_hands_the_host_a_typed_message() {
    let mut script = Script::default();
    run_on(&guest(LIST_AGENTS, SEND_LIST_AGENTS), &mut script)
        .await
        .unwrap();
    assert!(matches!(script.sent.as_slice(), [ClientMessage::ListAgents]));
}

#[tokio::test]
async fn send_of_json_that_is_no_client_message_is_a_decode_failure() {
    let wasm = guest(
        r#"{\"type\":\"et-bogus\"}"#,
        "(call $send (i32.const 0) (i32.const 19))",
    );
    let ended = run_on(&wasm, &mut Script::default()).await;
    assert!(matches!(ended, Err(CoreError::Guest(abi::DECODE))));
}

#[tokio::test]
async fn send_that_the_host_cannot_deliver_is_a_transport_failure() {
    let mut script = Script {
        send_error: Some(WsError::Transport("down".to_owned())),
        ..Script::default()
    };
    let ended = run_on(&guest(LIST_AGENTS, SEND_LIST_AGENTS), &mut script).await;
    assert!(matches!(ended, Err(CoreError::Guest(abi::TRANSPORT))));
}

#[tokio::test]
async fn recv_puts_the_message_json_in_the_slot() {
    // The message is taken into memory at 100, then stored under "k" in bucket "b" so the test can read it back.
    let wasm = guest(
        "bk",
        "(local $len i32) (local.set $len (call $recv (i32.const 100)))
        (drop (call $take (i32.const 100) (local.get $len)))
        (call $kv_set (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1) (i32.const 100) (local.get $len))",
    );
    let message = ServerMessage::RelayText {
        content: "hello".to_owned(),
    };
    let mut script = Script {
        inbox: VecDeque::from([Ok(Some(message.clone()))]),
        ..Script::default()
    };
    run_on(&wasm, &mut script).await.unwrap();
    assert_eq!(script.value("b", "k"), serde_json::to_vec(&message).unwrap());
}

#[tokio::test]
async fn recv_with_nothing_arrived_answers_nothing_and_a_failure_its_code() {
    let wasm = guest("", "(call $recv (i32.const 100))");
    run_on(&wasm, &mut Script::default()).await.unwrap();
    let decode = Err(WsError::Decode("bad".to_owned()));
    let failures = [
        (Err(WsError::Transport("down".to_owned())), abi::TRANSPORT),
        (decode, abi::DECODE),
        (Err(WsError::NotConnected), abi::NOT_CONNECTED),
    ];
    for (answer, code) in failures {
        let mut script = Script {
            inbox: VecDeque::from([answer]),
            ..Script::default()
        };
        let ended = run_on(&wasm, &mut script).await;
        assert!(matches!(ended, Err(CoreError::Guest(failure)) if failure == code));
    }
}

#[tokio::test]
async fn kv_get_finds_a_value_and_take_copies_it_only_into_a_buffer_it_fits() {
    // "value" is found under "k" in "b", a 2-byte take copies nothing, a 5-byte one copies it, and it is stored back
    // under "c" in "b".
    let wasm = guest(
        "bkc",
        "(if (i32.ne (call $kv_get (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1)) (i32.const 1))
          (then (return (i32.const 10))))
        (if (i32.ne (call $take (i32.const 100) (i32.const 2)) (i32.const 5)) (then (return (i32.const 11))))
        (if (i32.ne (call $take (i32.const 100) (i32.const 5)) (i32.const 5)) (then (return (i32.const 12))))
        (call $kv_set (i32.const 0) (i32.const 1) (i32.const 2) (i32.const 1) (i32.const 100) (i32.const 5))",
    );
    let mut script = Script::default();
    let _none = script
        .stored
        .insert(("b".to_owned(), "k".to_owned()), b"value".to_vec());
    run_on(&wasm, &mut script).await.unwrap();
    assert_eq!(script.value("b", "c"), b"value");
}

#[tokio::test]
async fn kv_get_of_an_absent_key_answers_absent() {
    let wasm = guest(
        "bk",
        "(call $kv_get (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))",
    );
    run_on(&wasm, &mut Script::default()).await.unwrap();
}

#[tokio::test]
async fn a_storage_failure_is_a_store_failure() {
    let getting = guest(
        "bk",
        "(call $kv_get (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))",
    );
    let setting = guest(
        "bk",
        "(call $kv_set (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1) (i32.const 0) (i32.const 2))",
    );
    for wasm in [getting, setting] {
        let mut script = Script {
            store_fails: true,
            ..Script::default()
        };
        let ended = run_on(&wasm, &mut script).await;
        assert!(matches!(ended, Err(CoreError::Guest(abi::STORE))));
    }
}

#[tokio::test]
async fn log_records_each_level() {
    let wasm = guest(
        "cm",
        "(call $log (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (call $log (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (call $log (i32.const 2) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (call $log (i32.const 3) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (call $log (i32.const 4) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (call $log (i32.const 5) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (i32.const 0)",
    );
    let mut script = Script::default();
    run_on(&wasm, &mut script).await.unwrap();
    let levels: Vec<Level> = script.logged.iter().map(|(level, ..)| *level).collect();
    let expected = [
        Level::Trace,
        Level::Debug,
        Level::Info,
        Level::Warn,
        Level::Error,
        Level::Critical,
    ];
    assert_eq!(levels, expected);
    assert!(
        script
            .logged
            .iter()
            .all(|(_, context, message)| context == "c" && message == "m")
    );
}

#[tokio::test]
async fn disconnect_closes_the_websocket() {
    let mut script = Script::default();
    run_on(&guest("", "(call $disconnect) (i32.const 0)"), &mut script)
        .await
        .unwrap();
    assert!(script.disconnected);
}

#[tokio::test]
async fn a_call_the_host_cannot_read_traps_the_guest() {
    let calls = [
        // A message past the end of the guest's one page of memory.
        "(call $send (i32.const 70000) (i32.const 4))",
        // A bucket name that is not UTF-8.
        "(call $kv_get (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))",
        // A log level that does not exist.
        "(call $log (i32.const 9) (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 1)) (i32.const 0)",
        // A buffer that fits the slot, past the end of memory.
        "(drop (call $agent_id)) (call $take (i32.const 70000) (i32.const 100))",
    ];
    for instructions in calls {
        let ended = run_on(&guest("\\ff", instructions), &mut Script::default()).await;
        assert!(matches!(ended, Err(CoreError::Trap(TrapReason::Host))));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_run_stops_its_module_and_releases_its_host() {
    // One guest computes forever and one waits forever on the host; dropping the run has to stop either.
    let stuck = [
        "(loop $forever (br $forever)) (i32.const 0)",
        "(call $recv (i32.const 100))",
    ];
    for instructions in stuck {
        let (released, dropped) = oneshot::channel::<()>();
        let run = run_core_module(guest("", instructions), Held { _released: released });
        let _gave_up: Elapsed = tokio::time::timeout(Duration::from_millis(200), run).await.unwrap_err();
        let _released: RecvError = tokio::time::timeout(Duration::from_secs(10), dropped)
            .await
            .unwrap()
            .unwrap_err();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_core_module_reaches_a_real_hub_through_the_runners_host() {
    // Connect, send `et-list-agents`, take the ack the hub registered the guest with, store "v" under "k" in the
    // guest's own bucket, read it back, and disconnect, failing with a distinct code at each step that goes wrong.
    let wasm = guest(
        &format!("kv{LIST_AGENTS}"),
        "(local $len i32)
        (if (call $connect) (then (return (i32.const 10))))
        (if (call $send (i32.const 2) (i32.const 25)) (then (return (i32.const 11))))
        (if (i32.eqz (call $recv (i32.const 10000))) (then (return (i32.const 12))))
        (local.set $len (call $agent_id))
        (drop (call $take (i32.const 1000) (local.get $len)))
        (if (call $kv_set (i32.const 1000) (local.get $len) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
          (then (return (i32.const 13))))
        (if (i32.ne (call $kv_get (i32.const 1000) (local.get $len) (i32.const 0) (i32.const 1)) (i32.const 1))
          (then (return (i32.const 14))))
        (if (i32.ne (call $take (i32.const 2000) (i32.const 1)) (i32.const 1)) (then (return (i32.const 15))))
        (if (i32.ne (i32.load8_u (i32.const 2000)) (i32.const 118)) (then (return (i32.const 16))))
        (call $log (i32.const 2) (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1))
        (call $disconnect)
        (i32.const 0)",
    );
    let server = et_ws_test_server::start();
    let http_base = et_ws_runner_common::derive_http_base(&server.ws_url).unwrap();
    let host = HostState::new(&http_base, server.ws_url.clone(), None, false);
    run(&wasm, host).await.unwrap();
}
