//! except1: demonstrates Rust's exception model in a browser WASM module.
//!
//! Rust has no throw/catch. Recoverable failures are `Result` values, handled where they occur or propagated
//! with `?`; the `Err` a `#[wasm_bindgen]` entry point returns becomes a rejected promise the JS host catches.
//! A `panic!` on wasm32-unknown-unknown does not unwind at all: it lowers to the `unreachable` instruction,
//! the instance traps, and the JS host observes an unrecoverable `RuntimeError` -- `std::panic::catch_unwind`
//! cannot intercept it on this target. The whole demo is therefore `Result`-shaped: an Ok path, a recovered
//! Err path, and boundary translation of the domain error into a `JsValue`.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use core::fmt;

use et_web::{sleep_ms, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea, wait_for_connected};
use tracing::info;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("except1 exception-handling demo module initialized");
}

/// Failure of [`checked_divide`]: the quotient is unrepresentable (zero divisor, or `i32::MIN / -1`).
#[derive(Debug, PartialEq, Eq)]
struct DivideError;

impl fmt::Display for DivideError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("quotient is unrepresentable (zero divisor or i32::MIN / -1)")
    }
}

/// Returns the quotient, or a [`DivideError`] the caller must consume.
///
/// The unignorable `Result` is the compiler-enforced analog of the C++ `throw` in zig-except1's
/// checked_divide.
fn checked_divide(num: i32, den: i32) -> Result<i32, DivideError> {
    num.checked_div(den).ok_or(DivideError)
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    let msg = "except1: entered run()";
    log(msg);
    set_module_status(msg)?;

    let ws_url = websocket_url()?;
    let mut client = WsClient::new(WsClientConfig::new(ws_url));

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    let msg = format!("except1: connected as {agent_id}");
    log(&msg);
    set_module_status(&msg)?;

    // Ok path: the value flows out of the Result exactly where the caller consumes it.
    let quotient = checked_divide(84, 4);
    let msg = match quotient {
        Ok(value) => format!("except1: checked_divide(84, 4) = {value} (Ok path)"),
        Err(ref error) => format!("except1: checked_divide(84, 4) failed unexpectedly: {error}"),
    };
    log(&msg);
    set_module_status(&msg)?;

    // Recovered Err path: the zero divisor produces an Err the caller handles in place and execution
    // continues -- Rust's analog of zig-except1's throw-caught-in-C++ demo.
    let recovered = checked_divide(1, 0);
    let msg = match recovered {
        Ok(value) => format!("except1: checked_divide(1, 0) unexpectedly returned {value}"),
        Err(ref error) => format!("except1: checked_divide(1, 0) recovered from error: {error}"),
    };
    log(&msg);
    set_module_status(&msg)?;

    if quotient == Ok(21) && recovered.is_err() {
        let msg = "except1: VERIFICATION SUCCESS - Result handling behaved as expected!";
        log(msg);
        set_module_status(msg)?;
    } else {
        let msg = "except1: VERIFICATION FAILURE - unexpected results!";
        log(msg);
        set_module_status(msg)?;
        // Boundary translation: the domain failure leaves the module as a JsValue, rejecting the promise the
        // JS host awaits -- the same boundary role as zig-except1's non-zero run() status code.
        return Err(JsValue::from_str("except1: unexpected Result outcomes"));
    }

    sleep_ms(2000).await?;
    client.disconnect();
    let msg = "except1: workflow complete";
    log(msg);
    set_module_status(msg)?;
    Ok(())
}

fn log(message: &str) {
    let line = format!("[except1] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    append_to_textarea("module-output", message)
}

async fn wait_for_agent_id(client: &WsClient) -> Result<String, JsValue> {
    for _ in 0_u32..100 {
        let agent_id = client.get_agent_id();
        if !agent_id.is_empty() {
            return Ok(agent_id);
        }
        sleep_ms(100).await?;
    }
    Err(JsValue::from_str("Timed out waiting for assigned agent_id"))
}
