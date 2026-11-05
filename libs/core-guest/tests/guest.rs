//! Covers the typed guest API over a stand-in for the `et` imports: every call, each code an import answers with, and
//! copying the host's result slot out.
#![cfg(test)]

use std::collections::VecDeque;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_core_guest::{Guest, GuestError, Imports, Level, abi};

/// The imports as a script: what each call answers, what the slot holds, and a record of what the guest did.
#[derive(Default)]
struct Script {
    /// What each call answers in order, a `take` included only when the slot does not fit its buffer.
    answers: VecDeque<i32>,
    /// The host's result slot.
    slot: Vec<u8>,
    /// What a `take` that copies answers instead of the slot's length, when set.
    copied: Option<i32>,
    /// Every call made other than `take` and `log`, by import name.
    calls: Vec<&'static str>,
    /// Every message sent.
    sent: Vec<Vec<u8>>,
    /// Every value stored, with its bucket and key.
    stored: Vec<(String, String, Vec<u8>)>,
    /// Every log entry: level, context, message.
    logged: Vec<(i32, String, String)>,
}

impl Script {
    /// A script answering `answers` in order, with `slot` in the result slot.
    fn new(answers: &[i32], slot: &[u8]) -> Self {
        Self {
            answers: answers.iter().copied().collect(),
            slot: slot.to_vec(),
            ..Self::default()
        }
    }

    /// The next answer, recording the call `name`.
    fn answer(&mut self, name: &'static str) -> i32 {
        self.calls.push(name);
        self.answers.pop_front().unwrap()
    }

    /// A guest over this script, logging as `twin`; the script is read back once the guest is done with it.
    fn guest(&mut self) -> Guest<&mut Self> {
        Guest::new(self, "twin")
    }
}

impl Imports for &mut Script {
    fn connect(&mut self) -> i32 {
        self.answer(abi::CONNECT)
    }

    fn agent_id(&mut self) -> i32 {
        self.answer(abi::AGENT_ID)
    }

    fn send(&mut self, message: &[u8]) -> i32 {
        self.sent.push(message.to_vec());
        self.answer(abi::SEND)
    }

    fn recv(&mut self, _timeout_ms: u32) -> i32 {
        self.answer(abi::RECV)
    }

    /// Copy the slot out when `buffer` holds it; otherwise the next scripted answer, or the slot's length.
    fn take(&mut self, buffer: &mut [u8]) -> i32 {
        let len = i32::try_from(self.slot.len()).unwrap();
        if let Some(target) = buffer.get_mut(..self.slot.len()) {
            target.copy_from_slice(&self.slot);
            return self.copied.unwrap_or(len);
        }
        self.answers.pop_front().unwrap_or(len)
    }

    fn disconnect(&mut self) {
        self.calls.push(abi::DISCONNECT);
    }

    fn kv_get(&mut self, _bucket: &str, _key: &str) -> i32 {
        self.answer(abi::KV_GET)
    }

    fn kv_set(&mut self, bucket: &str, key: &str, value: &[u8]) -> i32 {
        self.stored.push((bucket.to_owned(), key.to_owned(), value.to_vec()));
        self.answer(abi::KV_SET)
    }

    fn log(&mut self, level: i32, context: &str, message: &str) {
        self.logged.push((level, context.to_owned(), message.to_owned()));
    }
}

#[test]
fn start_logs_connects_and_returns_the_agent_id() {
    let mut script = Script::new(&[abi::SUCCESS, 7_i32], b"agent-1");
    assert_eq!(script.guest().start().unwrap(), "agent-1");
    assert_eq!(script.calls, [abi::CONNECT, abi::AGENT_ID]);
    assert_eq!(
        script.logged,
        [(abi::LOG_INFO, "twin".to_owned(), "entered run()".to_owned())]
    );
}

#[test]
fn each_failure_code_names_its_error() {
    let cases = [
        (abi::TRANSPORT, "the websocket failed, or the hub could not be reached"),
        (abi::DECODE, "the host could not decode the message"),
        (abi::NOT_CONNECTED, "the websocket is not open"),
        (abi::ALREADY_CONNECTED, "the websocket is already open"),
        (abi::STORE, "the hub's storage refused the request, or failed"),
        (-99_i32, "the host answered -99, which no `et` import returns"),
    ];
    for (code, text) in cases {
        let error = Script::new(&[code], b"").guest().connect().unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn an_agent_id_that_is_not_utf8_fails() {
    let error = Script::new(&[2_i32], &[0xff, 0xfe]).guest().agent_id().unwrap_err();
    assert!(matches!(error, GuestError::Utf8(_)));
    assert_eq!(error.code(), abi::FAILED);
}

#[test]
fn send_hands_the_host_the_message_json() {
    let message = ClientMessage::RelayText {
        content: "hello".to_owned(),
    };
    let mut script = Script::new(&[abi::SUCCESS, abi::NOT_CONNECTED], b"");
    let mut guest = script.guest();
    guest.send(&message).unwrap();
    assert!(matches!(guest.send(&message), Err(GuestError::NotConnected)));
    assert_eq!(script.sent.first().unwrap(), &serde_json::to_vec(&message).unwrap());
}

#[test]
fn recv_reads_the_message_in_the_slot() {
    let message = ServerMessage::RelayText {
        content: "hello".to_owned(),
    };
    let json = serde_json::to_vec(&message).unwrap();
    let len = i32::try_from(json.len()).unwrap();
    let mut script = Script::new(&[len], &json);
    let received = script.guest().recv(100).unwrap();
    assert!(matches!(received, Some(ServerMessage::RelayText { content }) if content == "hello"));
}

#[test]
fn recv_with_nothing_arrived_is_none() {
    assert!(Script::new(&[abi::NOTHING], b"").guest().recv(100).unwrap().is_none());
}

#[test]
fn recv_of_a_slot_that_is_not_a_message_fails() {
    let error = Script::new(&[3_i32], b"???").guest().recv(100).unwrap_err();
    assert!(matches!(error, GuestError::Json(_)));
    assert_eq!(error.code(), abi::FAILED);
}

#[test]
fn get_reads_a_found_value_and_reports_an_absent_one() {
    assert_eq!(
        Script::new(&[abi::FOUND], b"value")
            .guest()
            .get("bucket", "key")
            .unwrap()
            .unwrap(),
        b"value"
    );
    assert!(
        Script::new(&[abi::ABSENT], b"")
            .guest()
            .get("bucket", "key")
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        Script::new(&[abi::STORE], b"").guest().get("bucket", "key"),
        Err(GuestError::Store)
    ));
}

#[test]
fn set_stores_the_value_under_its_key() {
    let mut script = Script::new(&[abi::SUCCESS, abi::STORE], b"");
    let mut guest = script.guest();
    guest.set("bucket", "key", b"value").unwrap();
    assert!(matches!(guest.set("bucket", "key", b"value"), Err(GuestError::Store)));
    let first = script.stored.first().unwrap();
    assert_eq!(first, &("bucket".to_owned(), "key".to_owned(), b"value".to_vec()));
}

#[test]
fn disconnect_closes_the_websocket() {
    let mut script = Script::new(&[], b"");
    script.guest().disconnect();
    assert_eq!(script.calls, [abi::DISCONNECT]);
}

#[test]
fn each_level_logs_with_its_code() {
    let levels = [
        (Level::Trace, abi::LOG_TRACE),
        (Level::Debug, abi::LOG_DEBUG),
        (Level::Info, abi::LOG_INFO),
        (Level::Warn, abi::LOG_WARN),
        (Level::Error, abi::LOG_ERROR),
        (Level::Critical, abi::LOG_CRITICAL),
    ];
    for (level, code) in levels {
        let mut script = Script::new(&[], b"");
        script.guest().log(level, "message");
        assert_eq!(script.logged, [(code, "twin".to_owned(), "message".to_owned())]);
    }
}

#[test]
fn finish_returns_ok_for_success_and_logs_and_codes_a_failure() {
    let mut script = Script::new(&[], b"");
    let mut guest = script.guest();
    assert_eq!(guest.finish(Ok(())), abi::SUCCESS);
    assert_eq!(
        guest.finish(Err(GuestError::Runtime("no input".to_owned()))),
        abi::FAILED
    );
    assert_eq!(guest.finish(Err(GuestError::Transport)), abi::TRANSPORT);
    assert_eq!(
        script.logged.first().unwrap(),
        &(abi::LOG_ERROR, "twin".to_owned(), "no input".to_owned())
    );
}

#[test]
fn a_failed_length_probe_fails_the_take() {
    // `agent_id` answers the id's length, then the probe for the slot's length answers a failure instead.
    let mut script = Script::new(&[2_i32, abi::DECODE], b"id");
    assert!(matches!(script.guest().agent_id(), Err(GuestError::Decode)));
}

#[test]
fn a_take_that_copies_a_different_length_fails() {
    let mut script = Script::new(&[2_i32], b"id");
    script.copied = Some(1_i32);
    assert!(matches!(script.guest().agent_id(), Err(GuestError::Unknown(1_i32))));
}
