//! `PyTorch` counterpart to the wasi-graphics-info ML test: launch
//! et-ws-pyo3-runner with `torch_inference.py`, trigger it, and verify the
//! torch matmul + tiny-classifier round-trip.
//!
//! `torch` is declared `pipx:torch` in the python-only mise config, so it's
//! absent from a default `mise install`. The runner only puts it on `sys.path`
//! when it's among the current mise packages (via
//! `edge_toolkit::config::mise_python_site_packages`), so this test SKIPS unless
//! torch is reachable there -- run it under `MISE_ENV=python` after
//! `mise install pipx:torch`. When present, the control client broadcasts a
//! trigger; the module runs the workflow and returns a JSON summary we assert on.

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::single_call_fn,
    clippy::print_stderr,
    reason = "integration test: poll-loop math, single-use helpers, eprintln skip notice"
)]

use std::error::Error;
use std::process::{Command, Stdio};
use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::{connect_async, tungstenite};

type ControlSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Any non-protocol text frame triggers the module's workflow.
const TRIGGER: &str = "run-torch";

/// True when `pipx:torch` is reachable on a mise package `site-packages` -- the
/// exact condition under which the runner can `import torch`. Checking it here
/// keeps the skip decision identical to the runner's own capability.
fn torch_reachable() -> bool {
    edge_toolkit::config::mise_python_site_packages()
        .iter()
        .any(|site_packages| site_packages.join("torch").is_dir())
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
        let tungstenite::Message::Text(text) = frame? else {
            continue;
        };
        if let Ok(ServerMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<ServerMessage>(&text) {
            return Ok((socket, agent_id));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn torch_module_runs_inference() -> Result<(), Box<dyn Error>> {
    if !torch_reachable() {
        eprintln!("skipping torch_inference: pipx:torch not on any mise site-packages");
        eprintln!("  install with `MISE_ENV=python mise install pipx:torch` and re-run under MISE_ENV=python");
        return Ok(());
    }

    let server = et_ws_test_server::start();
    let (mut control, control_id) = control_client(&server.ws_url).await?;

    let module_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("RUNNER_MODULE", "torch_inference")
        .env("PYO3_PYTHONPATH", &module_path)
        .env("WS_SERVER_URL", &server.ws_url)
        .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    // torch import + first op can be slow on a cold interpreter, so give the
    // round-trip a generous budget.
    let result = tokio::time::timeout(Duration::from_mins(1), torch_round_trip(&mut control, &control_id)).await;

    drop(runner.kill());
    drop(runner.wait());

    let reply = result??;
    let parsed: serde_json::Value = match serde_json::from_str(&reply) {
        Ok(value) => value,
        Err(e) => return Err(format!("reply not JSON: {e}: {reply}").into()),
    };
    if parsed.get("framework").and_then(serde_json::Value::as_str) != Some("torch") {
        return Err(format!("unexpected framework in {reply}").into());
    }
    let c00 = parsed
        .get("matmul_c00")
        .and_then(serde_json::Value::as_f64)
        .ok_or("matmul_c00 missing")?;
    if (c00 - 2.0_f64).abs() > 1e-4_f64 {
        return Err(format!("matmul_c00 {c00} != 2.0").into());
    }
    if parsed.get("predicted_class").and_then(serde_json::Value::as_i64) != Some(3_i64) {
        return Err(format!("predicted_class != 3 in {reply}").into());
    }
    Ok(())
}

/// Poll `list_agents` until the runner registers, then broadcast `TRIGGER` and
/// return the first non-protocol text frame (the module's JSON summary).
async fn torch_round_trip(control: &mut ControlSocket, self_id: &str) -> Result<String, Box<dyn Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut have_peer = false;
    while std::time::Instant::now() < deadline {
        let req = serde_json::to_string(&ClientMessage::ListAgents)?;
        control.send(tungstenite::Message::Text(req)).await?;
        let poll_deadline = std::time::Instant::now() + Duration::from_millis(250);
        while std::time::Instant::now() < poll_deadline {
            let remaining = poll_deadline - std::time::Instant::now();
            match tokio::time::timeout(remaining, control.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                    if let Ok(ServerMessage::ListAgentsResponse { agents }) =
                        serde_json::from_str::<ServerMessage>(&text)
                        && agents.iter().any(|summary| summary.agent_id != self_id)
                    {
                        have_peer = true;
                        break;
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => break,
            }
        }
        if have_peer {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !have_peer {
        return Err("runner never registered".into());
    }

    control.send(tungstenite::Message::Text(TRIGGER.to_string())).await?;

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let remaining = deadline - std::time::Instant::now();
        match tokio::time::timeout(remaining, control.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                if serde_json::from_str::<ServerMessage>(&text).is_ok() {
                    // typed et-* envelope (status / list / ack), keep draining
                    continue;
                }
                return Ok(text);
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => return Err(format!("recv error: {e}").into()),
            Ok(None) => return Err("control socket closed".into()),
            Err(_) => return Err("timed out waiting for torch reply".into()),
        }
    }
    Err("deadline exceeded".into())
}
