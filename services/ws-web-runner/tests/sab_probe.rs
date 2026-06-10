//! Probe whether the embedded V8 supports `SharedArrayBuffer` + `Atomics` at all.
//!
//! Separate from the `Worker` question: if this passes, the only missing piece
//! for `et-ws-zig-data1` is the `Worker` constructor + the cross-thread
//! `BackingStore` sharing it needs.

#![cfg(test)]
#![expect(clippy::expect_used, reason = "probe test; should fail loudly if V8 lacks SAB")]

use deno_core::{JsRuntime, RuntimeOptions};

#[test]
fn shared_array_buffer_and_atomics_available() {
    let mut runtime = JsRuntime::new(RuntimeOptions::default());
    drop(
        runtime
            .execute_script(
                "<sab-probe>",
                r#"
const sab = new SharedArrayBuffer(64);
const view = new Int32Array(sab);
Atomics.store(view, 0, 42);
const got = Atomics.load(view, 0);
if (got !== 42) throw new Error("Atomics roundtrip failed: " + got);
if (typeof Atomics.wait !== "function") throw new Error("Atomics.wait missing");
if (typeof Atomics.notify !== "function") throw new Error("Atomics.notify missing");
"#,
            )
            .expect("SAB/Atomics probe should succeed"),
    );
}
