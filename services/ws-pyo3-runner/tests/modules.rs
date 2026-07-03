//! Integration tests: start an in-process et-ws-server and exercise each
//! bundled Python module through `et-ws-pyo3-runner`. Mirrors the single
//! `tests/modules.rs` the other runners use (`services/ws-wasi-runner`,
//! `services/ws-web-runner`), but the pyo3 modules are long-lived agents the
//! test drives over WebSocket rather than run-and-exit components, so each
//! case carries its own trigger + expectation via [`Exchange`].
//!
//! Every case shares one control client, one `list_agents` peer-wait, one
//! spawn path, and one module-directory helper. `no_hooks` is the exception --
//! it needs no server and asserts a non-zero exit -- so it is a separate test.

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::float_arithmetic,
    clippy::print_stderr,
    clippy::single_call_fn,
    reason = "integration test: poll math, float tolerance check, spawn expects, skip notices, step helpers"
)]

use std::error::Error;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use edge_toolkit::config::{Language, mise_env_includes};
use edge_toolkit::ws::{ClientMessage, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use rstest::rstest;
use tokio_tungstenite::{connect_async, tungstenite};

type ControlSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// When a case may not be runnable, the condition under which it self-skips.
enum Gate {
    /// Always runs (needs only the embedded interpreter + bundled module).
    Always,
    /// Needs the `python` mise env loaded (interpreter + its site-packages).
    Python,
    /// Needs `python` loaded *and* `pipx:torch` reachable on a site-packages dir.
    Torch,
}

/// A module's trigger frame and the assertion made on the reply.
enum Exchange {
    /// Broadcast `send`; expect the reply to contain `send` plus every `extra` needle.
    /// `extra: &[]` is a plain round-trip (echo); extra needles prove a transform ran (cowsay).
    TextContains {
        send: &'static str,
        extra: &'static [&'static str],
    },
    /// Broadcast text; parse the reply as JSON and run the checker.
    TextJson(&'static str, fn(&serde_json::Value) -> Result<(), Box<dyn Error>>),
    /// Send `key\0value` then `key`; expect the binary reply to equal `value`.
    StoragePutGet { key: &'static str, value: &'static [u8] },
    /// Send `[count]`; expect exactly `count` one-byte frames `0..count`.
    Fanout(u8),
}

#[rstest]
#[case::echo(
    "echo",
    Gate::Always,
    Exchange::TextContains { send: r#"{"hello":"world","from":"control"}"#, extra: &[] }
)]
#[case::storage(
    "storage_pingpong",
    Gate::Always,
    Exchange::StoragePutGet { key: "hello.txt", value: b"a quick brown fox jumps over the lazy dog" }
)]
#[case::fanout("fanout", Gate::Always, Exchange::Fanout(5))]
// cowsay reuses echo's Exchange: the payload must round-trip *and* carry the cow's `^__^`,
// proving the module imported the mise-provided cowsay and rendered our input.
#[case::cowsay("cowsay_probe", Gate::Python, Exchange::TextContains { send: "cowsay-round-trip", extra: &["^__^"] })]
#[case::torch("torch_inference", Gate::Torch, Exchange::TextJson("run-torch", check_torch))]
#[tokio::test(flavor = "current_thread")]
async fn module_behaves(
    #[case] module: &str,
    #[case] gate: Gate,
    #[case] exchange: Exchange,
) -> Result<(), Box<dyn Error>> {
    if skipped(module, &gate) {
        return Ok(());
    }

    let server = et_ws_test_server::start();
    // Control client registers first so the runner has a peer to broadcast to.
    let (mut control, control_id) = control_client(&server.ws_url).await?;
    let mut runner = spawn_runner(module, &server.ws_url);

    // torch's cold import + first op is slow; the rest are quick.
    let budget = if matches!(gate, Gate::Torch) {
        Duration::from_mins(1)
    } else {
        Duration::from_secs(30)
    };
    let outcome = tokio::time::timeout(budget, run_exchange(&mut control, &control_id, &exchange)).await;

    drop(runner.kill());
    drop(runner.wait());

    match outcome {
        Ok(result) => result,
        Err(_elapsed) => Err(format!("`{module}` exchange timed out").into()),
    }
}

/// Load-time sanity check: a module defining none of the runner hooks must fail
/// to load (the import happens in `initialize`, before any connection), so no
/// server is needed and the runner exits non-zero.
#[test]
fn no_hooks_fails_to_load() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_et-ws-pyo3-runner"))
        .env("RUNNER_MODULE", "no_hooks")
        .env("PYO3_PYTHONPATH", python_dir())
        // Safety net: if the check regressed and import succeeded, bound the
        // otherwise-forever connect retry so the assertion below fires instead.
        .env("RUNNER_TIMEOUT", "10s")
        .env("RUST_LOG", "error")
        .output()?;

    if output.status.success() {
        return Err(format!("a hookless module must fail to load; got {:?}", output.status).into());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("none of the runner hooks") {
        return Err(format!("stderr should explain the missing hooks; got: {stderr}").into());
    }
    Ok(())
}

/// True if `gate` isn't satisfied on this host (emitting a skip note for torch).
fn skipped(module: &str, gate: &Gate) -> bool {
    match gate {
        Gate::Always => false,
        Gate::Python => !mise_env_includes(Language::Python),
        Gate::Torch => {
            if !mise_env_includes(Language::Python) {
                return true;
            }
            if !torch_reachable() {
                eprintln!("skipping {module}: pipx:torch not on any mise site-packages");
                eprintln!("  install with `MISE_ENV=python mise install pipx:torch` and re-run under MISE_ENV=python");
                return true;
            }
            false
        }
    }
}

/// Send the module's trigger and assert on its reply.
async fn run_exchange(control: &mut ControlSocket, self_id: &str, exchange: &Exchange) -> Result<(), Box<dyn Error>> {
    wait_for_peer(control, self_id).await?;
    match exchange {
        Exchange::TextContains { send, extra } => {
            control.send(tungstenite::Message::Text(send.to_string())).await?;
            let reply = drain_text(control).await?;
            if !reply.contains(send) {
                return Err(format!("reply {reply:?} did not contain the sent {send:?}").into());
            }
            for needle in *extra {
                if !reply.contains(needle) {
                    return Err(format!("reply {reply:?} is missing {needle:?}").into());
                }
            }
        }
        Exchange::TextJson(payload, check) => {
            control.send(tungstenite::Message::Text(payload.to_string())).await?;
            let reply = drain_text(control).await?;
            let value: serde_json::Value = match serde_json::from_str(&reply) {
                Ok(value) => value,
                Err(err) => return Err(format!("reply not JSON: {err}: {reply}").into()),
            };
            check(&value)?;
        }
        Exchange::StoragePutGet { key, value } => {
            let mut put_frame = Vec::with_capacity(key.len() + 1 + value.len());
            put_frame.extend_from_slice(key.as_bytes());
            put_frame.push(0);
            put_frame.extend_from_slice(value);
            control.send(tungstenite::Message::Binary(put_frame)).await?;
            // Let the storage worker PUT to disk before we GET.
            tokio::time::sleep(Duration::from_millis(200)).await;
            control
                .send(tungstenite::Message::Binary(key.as_bytes().to_vec()))
                .await?;
            let reply = drain_binary(control).await?;
            if reply.as_slice() != *value {
                return Err(format!("stored bytes {reply:?} did not round-trip").into());
            }
        }
        Exchange::Fanout(count) => {
            control.send(tungstenite::Message::Binary(vec![*count])).await?;
            let frames = collect_binary(control, usize::from(*count)).await?;
            let expected: Vec<u8> = (0..*count).collect();
            if frames != expected {
                return Err(format!("fan-out frames {frames:?} did not match {expected:?}").into());
            }
        }
    }
    Ok(())
}

/// Torch case checker: matmul + tiny-classifier summary from `torch_inference.py`.
fn check_torch(value: &serde_json::Value) -> Result<(), Box<dyn Error>> {
    if value.get("framework").and_then(serde_json::Value::as_str) != Some("torch") {
        return Err(format!("unexpected framework in {value}").into());
    }
    let c00 = value
        .get("matmul_c00")
        .and_then(serde_json::Value::as_f64)
        .ok_or("matmul_c00 missing")?;
    if (c00 - 2.0_f64).abs() > 1e-4_f64 {
        return Err(format!("matmul_c00 {c00} != 2.0").into());
    }
    if value.get("predicted_class").and_then(serde_json::Value::as_i64) != Some(3_i64) {
        return Err(format!("predicted_class != 3 in {value}").into());
    }
    Ok(())
}

/// Directory of the bundled test Python modules -- the single source of truth
/// for `PYO3_PYTHONPATH` across every case.
fn python_dir() -> PathBuf {
    edge_toolkit::config::get_project_root().join("services/ws-pyo3-runner/python")
}

/// Spawn the runner subprocess for `module`, pointed at `ws_url`.
fn spawn_runner(module: &str, ws_url: &str) -> Child {
    // Silence the runner unless invoked with --nocapture and RUST_LOG opted in.
    let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_string());
    Command::new(env!("CARGO_BIN_EXE_et-ws-pyo3-runner"))
        .env("RUNNER_MODULE", module)
        .env("PYO3_PYTHONPATH", python_dir())
        .env("WS_SERVER_URL", ws_url)
        .env("RUST_LOG", rust_log)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn et-ws-pyo3-runner")
}

/// Open a control client and drive et-connect until we have an `agent_id`.
async fn control_client(ws_url: &str) -> Result<(ControlSocket, String), Box<dyn Error>> {
    let (mut socket, _) = connect_async(ws_url).await?;
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id: None })?;
    socket.send(tungstenite::Message::Text(connect)).await?;
    loop {
        let Some(frame) = socket.next().await else {
            return Err("control socket closed before connect-ack".into());
        };
        if let tungstenite::Message::Text(text) = frame?
            && let Ok(ServerMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<ServerMessage>(&text)
        {
            return Ok((socket, agent_id));
        }
    }
}

/// Poll `list_agents` until a peer other than us (the runner) registers.
async fn wait_for_peer(control: &mut ControlSocket, self_id: &str) -> Result<(), Box<dyn Error>> {
    // The runner needs ~1s to spawn + init Python + connect; give it room.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let req = serde_json::to_string(&ClientMessage::ListAgents)?;
        control.send(tungstenite::Message::Text(req)).await?;
        // Drain responses for a short window before re-polling.
        let poll_until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < poll_until {
            let remaining = poll_until - Instant::now();
            match tokio::time::timeout(remaining, control.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                    if let Ok(ServerMessage::ListAgentsResponse { agents }) =
                        serde_json::from_str::<ServerMessage>(&text)
                        && agents.iter().any(|summary| summary.agent_id != self_id)
                    {
                        return Ok(());
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => break,
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("runner never registered".into())
}

/// Drain frames until the first non-protocol text frame (skipping typed et-* envelopes).
async fn drain_text(control: &mut ControlSocket) -> Result<String, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let remaining = deadline - Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                if serde_json::from_str::<ServerMessage>(&text).is_ok() {
                    continue; // typed et-* envelope (status / list / ack), keep draining
                }
                return Ok(text);
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(err))) => return Err(format!("recv error: {err}").into()),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for text reply".into()),
        }
    }
    Err("deadline exceeded waiting for text reply".into())
}

/// Drain frames until the first binary frame.
async fn drain_binary(control: &mut ControlSocket) -> Result<Vec<u8>, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let remaining = deadline - Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => return Ok(bytes),
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(err))) => return Err(format!("recv error: {err}").into()),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for binary reply".into()),
        }
    }
    Err("deadline exceeded waiting for binary reply".into())
}

/// Collect exactly `count` one-byte binary frames (ignoring typed et-* envelopes).
async fn collect_binary(control: &mut ControlSocket, count: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut received = Vec::with_capacity(count);
    let deadline = Instant::now() + Duration::from_secs(10);
    while received.len() < count && Instant::now() < deadline {
        let remaining = deadline - Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Binary(bytes)))) => {
                let [byte] = bytes.as_slice() else {
                    return Err(format!("fan-out produced a {}-byte frame, expected 1", bytes.len()).into());
                };
                received.push(*byte);
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(err))) => return Err(format!("recv error: {err}").into()),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for fan-out frames".into()),
        }
    }
    if received.len() != count {
        return Err(format!("got {} frames, expected {count}", received.len()).into());
    }
    Ok(received)
}

/// True when `pipx:torch` is importable from a mise package `site-packages` --
/// the exact condition under which the runner can `import torch`.
fn torch_reachable() -> bool {
    edge_toolkit::config::mise_python_site_packages()
        .iter()
        .any(|site_packages| site_packages.join("torch").is_dir())
}
