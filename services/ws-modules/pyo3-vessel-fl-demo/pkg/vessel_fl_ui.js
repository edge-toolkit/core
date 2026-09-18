// Shared browser controller/view for the native vessel FL agents.

export function createVesselFlUi(config) {
  let wasmAgent = null;
  let client = null;
  let running = false;
  let firstLine = true;
  let startAcknowledged = false;
  let statusReceived = false;
  let retryTimer = null;
  const seenEvents = new Set();
  const jobId = globalThis.__VESSEL_FL_JOB_ID || "vessel-fl-demo";

  function appendOutput(message) {
    const output = document.getElementById("module-output");
    if (!output) return;
    const line = `[${config.label}] ${message}`;
    if (firstLine || !output.value || output.value.startsWith("Workflow module")) {
      output.value = line;
      firstLine = false;
    } else {
      output.value += `\n${line}`;
    }
    output.scrollTop = output.scrollHeight;
    console.log(line);
  }

  const percent = (value) => (Number.isFinite(value) ? `${(value * 100).toFixed(1)}%` : "n/a");
  const decimal = (value) => (Number.isFinite(value) ? value.toFixed(4) : "n/a");
  const roundLabel = (value) => (Number.isInteger(value) ? value + 1 : "?");

  function parsePayload(frame) {
    if (typeof frame !== "string") return null;
    try {
      const message = JSON.parse(frame);
      if (message?.type === "et-agent-message" && typeof message.message === "string") {
        return JSON.parse(message.message);
      }
      return message;
    } catch {
      return null;
    }
  }

  function eventKey(message) {
    if (message.type === "vessel-fl-round") return `${message.type}:${message.job_id}:${message.round}`;
    if (message.type === "vessel-fl-update") {
      return `${message.type}:${message.job_id}:${message.round}:${message.client_id}:${message.status}`;
    }
    if (message.type === "vessel-fl-client-status") {
      return `${message.type}:${message.client_id}:${message.round}:${message.status}`;
    }
    if (message.type === "vessel-fl-coordinator-status") {
      const received = (message.received_clients ?? []).join(",");
      return `${message.type}:${message.round}:${message.status}:${received}`;
    }
    if (message.type === "vessel-fl-complete") return `${message.type}:${message.job_id}`;
    if (message.type === "vessel-fl-status") return `${message.type}:${message.job_id}:${message.status}`;
    if (message.type === "mlflow-ack") return `${message.type}:${message.event_id}:${message.status}`;
    return null;
  }

  function showClientStatus(message) {
    if (message.client_id !== config.clientId) return;
    statusReceived = true;
    if (message.status === "dataset-loaded") {
      appendOutput(
        `native client ready: ${message.train_samples ?? "?"} training and ` +
          `${message.validation_samples ?? "?"} validation samples`,
      );
    } else if (message.status === "training") {
      appendOutput(`received global model; training round ${roundLabel(message.round)} locally`);
    } else if (message.status === "uploading-update") {
      appendOutput(`local training complete; uploading round ${roundLabel(message.round)} update`);
    } else if (message.status === "waiting") {
      const metrics = message.metrics ?? {};
      appendOutput(
        `update sent to coordinator: ${message.samples ?? "?"} samples, ` +
          `train accuracy ${percent(metrics.train_accuracy)}, ` +
          `validation accuracy ${percent(metrics.validation_accuracy)}`,
      );
    } else if (message.status === "failed") {
      appendOutput(`failed in round ${roundLabel(message.round)}: ${message.error ?? "unknown error"}`);
    }
  }

  function showCoordinatorStatus(message) {
    statusReceived = true;
    const received = message.received_clients ?? [];
    if (message.status === "waiting-for-start") {
      appendOutput(`native coordinator ready; expecting ${message.expected_clients ?? 3} clients`);
    } else if (message.status === "run-started") {
      appendOutput("start command received; federated run started");
    } else if (message.status === "round-started") {
      appendOutput(`round ${roundLabel(message.round)}: global model published; waiting for three clients`);
    } else if (message.status === "waiting-for-clients") {
      appendOutput(
        `round ${roundLabel(message.round)}: received ${received.length}/${message.expected_clients} updates ` +
          `(${received.join(", ")})`,
      );
    } else if (message.status === "aggregating") {
      appendOutput(`round ${roundLabel(message.round)}: all updates received; running weighted FedAvg`);
    } else if (message.status === "evaluating") {
      appendOutput(`round ${roundLabel(message.round)}: evaluating the global model`);
    } else if (message.status === "round-completed") {
      const metrics = message.metrics ?? {};
      appendOutput(
        `round ${roundLabel(message.round)} complete: global accuracy ` +
          `${percent(metrics.global_test_accuracy)}, loss ${decimal(metrics.global_test_loss)}`,
      );
    } else if (message.status === "run-completed") {
      appendOutput(`run complete: accuracy ${percent(message.metrics?.accuracy)}`);
    } else if (message.status === "failed") {
      appendOutput("federated run failed; inspect native coordinator logs");
    }
  }

  function showMessage(message) {
    if (!message || typeof message !== "object" || message.job_id !== jobId) return;
    const key = eventKey(message);
    if (key && seenEvents.has(key)) return;
    if (key) seenEvents.add(key);

    if (config.role === "client" && message.type === "vessel-fl-client-status") {
      showClientStatus(message);
      return;
    }
    if (config.role === "coordinator" && message.type === "vessel-fl-coordinator-status") {
      showCoordinatorStatus(message);
      return;
    }
    if (config.role === "client" && message.type === "vessel-fl-round") {
      appendOutput(`coordinator announced round ${roundLabel(message.round)}`);
      return;
    }
    if (config.role === "coordinator" && message.type === "vessel-fl-update") {
      appendOutput(
        `received round ${roundLabel(message.round)} update from ${message.client_id}: ` +
          `${message.samples ?? "?"} samples`,
      );
      return;
    }
    if (config.role === "coordinator" && message.type === "vessel-fl-status") {
      if (["accepted", "running", "completed", "already-started"].includes(message.status)) {
        startAcknowledged = true;
      }
      if (message.status === "accepted") appendOutput("browser start command accepted");
      else if (message.status === "already-started") appendOutput("the configured run has already started");
      return;
    }
    if (config.role === "coordinator" && message.type === "vessel-fl-complete") {
      appendOutput(`final model stored at /storage/${message.bucket}/${message.filename}`);
      return;
    }
    if (config.role === "coordinator" && message.type === "mlflow-ack") {
      appendOutput(`MLflow ${message.event_id ?? "event"}: ${message.status ?? "unknown"}`);
    }
  }

  function sleep(milliseconds) {
    return new Promise((resolve) => setTimeout(resolve, milliseconds));
  }

  async function waitForConnection() {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (client?.get_state() === "connected" && client.get_agent_id()) return;
      await sleep(100);
    }
    throw new Error("timeout waiting for vessel FL WebSocket connection");
  }

  function sendCommands() {
    if (client?.get_state() !== "connected") return;
    if (!statusReceived) {
      client.send(
        JSON.stringify({
          type: "vessel-fl-command",
          schema_version: 1,
          action: "status",
          job_id: jobId,
          role: config.role,
          client_id: config.clientId,
        }),
      );
    }
    if (config.role === "coordinator" && !startAcknowledged) {
      client.send(
        JSON.stringify({
          type: "vessel-fl-command",
          schema_version: 1,
          action: "start",
          job_id: jobId,
        }),
      );
    }
    if (statusReceived && (config.role !== "coordinator" || startAcknowledged)) {
      clearInterval(retryTimer);
      retryTimer = null;
    }
  }

  async function init() {
    wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
    await wasmAgent.default();
  }

  async function run() {
    if (!wasmAgent) throw new Error(`${config.label} is not initialized`);
    if (running) return;
    firstLine = true;
    startAcknowledged = false;
    statusReceived = false;
    seenEvents.clear();
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = globalThis.__ET_WS_URL || `${protocol}//${window.location.host}/ws`;
    const wsConfig = new wasmAgent.WsClientConfig(wsUrl);
    // The page shell already owns the origin's retained identity. Every role
    // view needs a separate ephemeral identity so it cannot steal that session.
    wsConfig.use_retained_agent_id = false;
    client = new wasmAgent.WsClient(wsConfig);
    client.set_on_message((frame) => showMessage(parsePayload(frame)));
    client.set_on_state_change((state) => {
      if (state === "connected") appendOutput("browser connected to the Edge Toolkit hub");
      if (state === "reconnecting") appendOutput("hub connection lost; reconnecting");
    });
    client.connect();
    running = true;
    try {
      await waitForConnection();
      appendOutput(config.role === "coordinator" ? "requesting FL start" : "requesting native client status");
      retryTimer = setInterval(sendCommands, 2000);
      sendCommands();
    } catch (error) {
      stop();
      throw error;
    }
  }

  function stop() {
    if (retryTimer !== null) clearInterval(retryTimer);
    retryTimer = null;
    if (client) client.disconnect();
    client = null;
    running = false;
    appendOutput("browser view stopped");
  }

  return { init, run, stop, isRunning: () => running };
}
