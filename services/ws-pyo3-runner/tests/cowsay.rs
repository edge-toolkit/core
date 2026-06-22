//! Proves the runner pre-populates `sys.path` with mise-installed pipx
//! packages: launch et-ws-pyo3-runner with `cowsay_probe.py`, which does a
//! top-level `import cowsay`, and verify a frame round-trips through cowsay.
//!
//! `cowsay` is declared as `pipx:cowsay` in the always-loaded mise config but
//! is NOT on `PYO3_PYTHONPATH` (which only points at the module dir). So the only
//! way the module imports is `edge_toolkit::config::mise_python_site_packages`
//! adding cowsay's venv `site-packages` to `sys.path`. The control client
//! broadcasts a plain string; the module returns the cowsay-rendered output;
//! we assert it came back transformed (contains the payload AND the cow art).

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::single_call_fn,
    reason = "integration test: Instant/Duration poll-loop math, single-use helpers"
)]

use std::error::Error;
use std::process::{Command, Stdio};
use std::time::Duration;

use edge_toolkit::config::{Language, mise_env_includes};
use edge_toolkit::ws::{ClientMessage, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::{connect_async, tungstenite};

type ControlSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A payload free of JSON punctuation (so the server treats it as an
/// unrecognised frame and broadcasts it) and free of `^__^` (so finding that
/// marker in the reply can only come from cowsay).
const PAYLOAD: &str = "split-learning-rocks";

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
async fn cowsay_module_imports_mise_package() -> Result<(), Box<dyn Error>> {
    // Although `pipx:cowsay` itself sits in the always-loaded base config,
    // the test exercises the Python interpreter wired up by the python env
    // (PYO3_PYTHON + the pyo3 runner's `mise_python_site_packages` lookup
    // both target the python toolchain). When CI narrows MISE_ENV to drop
    // `python`, the resolved interpreter / site-packages may be absent and
    // the runner can't import cowsay; skip cleanly.
    if !mise_env_includes(Language::Python) {
        return Ok(());
    }
    let server = et_ws_test_server::start();

    // Control client registers first so the runner has a peer to broadcast to.
    let (mut control, control_id) = control_client(&server.ws_url).await?;

    // Spawn the runner. PYO3_PYTHONPATH points only at the module dir (for
    // cowsay_probe.py) -- cowsay itself must come from the mise site-packages
    // the runner wires in, which is the whole point of the test.
    let module_path = format!("{}/python", env!("CARGO_MANIFEST_DIR"));
    let bin = env!("CARGO_BIN_EXE_et-ws-pyo3-runner");
    let mut runner = Command::new(bin)
        .env("RUNNER_MODULE", "cowsay_probe")
        .env("PYO3_PYTHONPATH", &module_path)
        .env("WS_SERVER_URL", &server.ws_url)
        .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    let result = tokio::time::timeout(Duration::from_secs(20), cowsay_round_trip(&mut control, &control_id)).await;

    drop(runner.kill());
    drop(runner.wait());

    let reply = result??;
    // cowsay wraps the payload in a speech bubble drawn above an ASCII cow; the
    // `^__^` is part of the cow and never appears in PAYLOAD, so its presence
    // proves the module both imported cowsay and ran it on our input.
    if !reply.contains(PAYLOAD) || !reply.contains("^__^") {
        return Err(format!("reply {reply:?} is not cowsay output for {PAYLOAD:?}").into());
    }
    Ok(())
}

/// Poll `list_agents` until the runner registers, then broadcast `PAYLOAD` and
/// return the first non-protocol text frame that comes back (the cowsay output).
async fn cowsay_round_trip(control: &mut ControlSocket, self_id: &str) -> Result<String, Box<dyn Error>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
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

    control.send(tungstenite::Message::Text(PAYLOAD.to_string())).await?;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
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
            Err(_) => return Err("timed out waiting for cowsay reply".into()),
        }
    }
    Err("deadline exceeded".into())
}
