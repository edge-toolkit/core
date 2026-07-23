import init, { initTracing, WsClient, WsClientConfig } from "/modules/et-ws-wasm-agent/et_ws_wasm_agent.js";

// Bump this string on every meaningful app.js edit. index.html loads this file via a plain, non-cache-busted
// <script src="/app.js">, so a client running stale code is otherwise invisible -- this value round-trips to
// the server tty (via the et-client-event sent below) so a stale load is diagnosable from server-side logs
// alone, without trusting what the client claims to be running.
const APP_JS_BUILD = "consent-log-v1";

console.log(`app.js: module loading started (build ${APP_JS_BUILD})`);

await new Promise((resolve, reject) => {
  const s = document.createElement("script");
  s.src = "/modules/onnxruntime-web/dist/ort.min.js";
  s.onload = resolve;
  s.onerror = reject;
  document.head.appendChild(s);
});

const logEl = document.getElementById("log");
const moduleSelect = document.getElementById("module-select");
const runModuleButton = document.getElementById("run-module-button");
const agentStatusEl = document.getElementById("agent-status");
const agentIdEl = document.getElementById("agent-id");
const uploadConsentCheckbox = document.getElementById("upload-consent");
const medicalNoteEl = document.querySelector(".medical-note");

// Set on the first "Run" click (of any module): hides the disclaimer for good and freezes the consent
// checkbox at whatever value it held at that moment, so a run's consent can't be changed after the fact.
let moduleHasRun = false;

const STORED_AGENT_ID_KEY = "et_ws_wasm_agent.agent_id";
let currentAgentId = null;

const append = (line) => {
  logEl.textContent += `\n${line}`;
};

const describeError = (error) => (error instanceof Error ? error.message : String(error));

const WORKFLOW_MODULES = new Map();
let activeWorkflow = null;
// Preselected in the dropdown when the server's module list includes it; otherwise the first option stays.
const DEFAULT_MODULE = "et-ws-pydemo1";

const populateModuleDropdown = async () => {
  append("Discovering modules via /modules...");
  const resp = await fetch("/modules/");
  if (!resp.ok) {
    append(`Failed to fetch module list from server: ${resp.status} ${resp.statusText}`);
    return;
  }
  const moduleNames = await resp.json();
  append(`Found ${moduleNames.length} potential modules: ${moduleNames.join(", ")}`);

  moduleSelect.innerHTML = "";

  for (const name of moduleNames) {
    try {
      if (name === "et-ws-wasm-agent") {
        append(`Skipping ${name}: already loaded as the main WASM agent module`);
        continue;
      }
      if (!name.startsWith("et-ws-")) {
        append(`Skipping ${name}: not an et-ws-* workflow module`);
        continue;
      }
      if (name.startsWith("et-ws-wasi-")) {
        append(`Skipping ${name}: WASI module, runs in et-ws-wasi-runner rather than the browser`);
        continue;
      }
      const pkgResp = await fetch(`/modules/${name}/package.json`, { cache: "no-cache" });
      if (!pkgResp.ok) {
        append(`Skipping ${name}: no package.json (${pkgResp.status})`);
        continue;
      }
      const pkg = await pkgResp.json();

      if (!pkg.main) {
        append(`Skipping ${name}: no main in package.json`);
        continue;
      }

      const moduleUrl = `/modules/${name}/${pkg.main}`;

      const label = pkg.description || pkg.name || name;
      WORKFLOW_MODULES.set(name, { label, moduleUrl, loaded: null });

      const option = document.createElement("option");
      option.value = name;
      option.textContent = label;
      moduleSelect.appendChild(option);

      append(`Discovered module: ${name} (${pkg.version})`);
    } catch (error) {
      append(`Error discovering module ${name}: ${describeError(error)}`);
      console.error(`discovery error for ${name}:`, error);
    }
  }

  if (WORKFLOW_MODULES.has(DEFAULT_MODULE)) moduleSelect.value = DEFAULT_MODULE;
};

const updateAgentCard = (status, agentId = currentAgentId) => {
  currentAgentId = agentId || null;
  agentStatusEl.textContent = status;
  agentIdEl.textContent = currentAgentId ?? "unassigned";
};

const readStoredAgentId = () => {
  try {
    return window.localStorage.getItem(STORED_AGENT_ID_KEY);
  } catch (error) {
    append(`agent storage read error: ${error instanceof Error ? error.message : String(error)}`);
    return null;
  }
};

const writeStoredAgentId = (agentId) => {
  try {
    window.localStorage.setItem(STORED_AGENT_ID_KEY, agentId);
  } catch (error) {
    append(`agent storage write error: ${error instanceof Error ? error.message : String(error)}`);
  }
};

const loadWorkflowModule = async (moduleKey) => {
  const moduleConfig = WORKFLOW_MODULES.get(moduleKey);
  if (!moduleConfig) {
    throw new Error(`unknown workflow module: ${moduleKey}`);
  }

  if (moduleConfig.loaded) {
    return moduleConfig.loaded;
  }

  const cacheBust = Date.now();
  const moduleUrl = `${moduleConfig.moduleUrl}?v=${cacheBust}`;
  append(`${moduleConfig.label} module: importing ${moduleUrl}`);
  const loadedModule = await import(moduleUrl);
  await loadedModule.default();
  moduleConfig.loaded = loadedModule;
  return loadedModule;
};

const runSelectedWorkflowModule = async () => {
  const moduleKey = moduleSelect.value;
  const moduleConfig = WORKFLOW_MODULES.get(moduleKey);
  if (!moduleConfig) {
    throw new Error(`unknown workflow module: ${moduleKey}`);
  }

  if (activeWorkflow && activeWorkflow.key !== moduleKey) {
    if (typeof activeWorkflow.module.stop === "function") {
      append(`${activeWorkflow.label} module: stopping before ${moduleConfig.label}`);
      await activeWorkflow.module.stop();
    }
    activeWorkflow = null;
  }

  const loadedModule = await loadWorkflowModule(moduleKey);
  if (
    typeof loadedModule.is_running === "function" &&
    loadedModule.is_running() &&
    typeof loadedModule.stop === "function"
  ) {
    append(`${moduleConfig.label} module: calling stop()`);
    loadedModule.stop();
    activeWorkflow = null;
    append(`${moduleConfig.label} module stopped`);
    return;
  }

  append(`${moduleConfig.label} module: calling run()`);
  const runPromise = loadedModule.run();
  append(`${moduleConfig.label} module: run() started`);
  await runPromise;
  append(`${moduleConfig.label} module run() returned`);
  if (typeof loadedModule.is_running === "function" && loadedModule.is_running()) {
    activeWorkflow = { key: moduleKey, label: moduleConfig.label, module: loadedModule };
  }
};

const handleProtocolMessage = (message) => {
  let parsed;

  try {
    parsed = JSON.parse(message);
  } catch {
    return;
  }

  if (parsed?.type !== "et-connect-ack" || typeof parsed.agent_id !== "string") {
    return;
  }

  writeStoredAgentId(parsed.agent_id);

  if (parsed.status === "reconnected") {
    updateAgentCard("Reconnected with previously issued server ID.", parsed.agent_id);
    append(`agent_id reused: ${parsed.agent_id}`);
    return;
  }

  updateAgentCard("Server assigned a new agent ID.", parsed.agent_id);
  append(`agent_id assigned: ${parsed.agent_id}`);
};

const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
const wsUrl = `${wsProtocol}//${window.location.host}/ws`;
const retainedAgentId = readStoredAgentId();

const wasmUrl = "/modules/et-ws-wasm-agent/et_ws_wasm_agent_bg.wasm";
logEl.textContent = `Initializing WASM from ${wasmUrl}\nWebSocket endpoint: ${wsUrl}`;
updateAgentCard(
  retainedAgentId
    ? "Found retained agent ID in local storage. It will be re-used on connect."
    : "No retained agent ID found. Waiting for server assignment.",
  retainedAgentId,
);

try {
  try {
    await populateModuleDropdown();
  } catch (error) {
    append(`Module discovery failed: ${describeError(error)}`);
    console.error("populateModuleDropdown error:", error);
  }

  await init();
  initTracing();

  const config = new WsClientConfig(wsUrl);
  const client = new WsClient(config);

  client.set_on_state_change((state) => {
    append(`state: ${state}`);
    if (state === "connecting") {
      if (uploadConsentCheckbox) uploadConsentCheckbox.disabled = true;
      updateAgentCard("Connecting to websocket server...", client.get_agent_id() || readStoredAgentId());
    } else if (state === "connected") {
      // Only enabled once the client can actually send: toggling it earlier would call client.send() on a
      // not-yet-connected client, which silently never reaches the server (the checkbox reads "checked" on
      // the page while the server never learns about it -- indistinguishable from the toggle just not
      // working at all). Skipped once a module has run: its consent choice is frozen from then on, so a
      // later reconnect must not re-enable it.
      if (uploadConsentCheckbox && !moduleHasRun) uploadConsentCheckbox.disabled = false;
      updateAgentCard(
        "Socket connected. Waiting for server identity acknowledgement...",
        client.get_agent_id() || readStoredAgentId(),
      );
      // Sent on every (re)connect, not just once: a reconnect after the server restarts is exactly when a
      // stale client would otherwise go unnoticed, since app.js itself never reloads on a WebSocket bounce.
      client.send(
        JSON.stringify({
          type: "et-client-event",
          capability: "app",
          action: "loaded",
          details: { build: APP_JS_BUILD },
        }),
      );
    } else if (state === "reconnecting") {
      if (uploadConsentCheckbox) uploadConsentCheckbox.disabled = true;
      updateAgentCard(
        "Disconnected. Trying to re-use retained agent ID...",
        client.get_agent_id() || readStoredAgentId(),
      );
    } else if (state === "disconnected") {
      if (uploadConsentCheckbox) uploadConsentCheckbox.disabled = true;
      updateAgentCard(
        "Socket disconnected. Retained agent ID will be re-used on next connect.",
        client.get_agent_id() || readStoredAgentId(),
      );
    }
  });
  client.set_on_message((message) => {
    append(`message: ${message}`);
    handleProtocolMessage(message);
  });

  client.connect();
  updateAgentCard(
    retainedAgentId
      ? "Attempting websocket connect with retained agent ID from local storage."
      : "Attempting first websocket connect. Waiting for server-assigned agent ID.",
    client.get_agent_id() || retainedAgentId,
  );
  append(`agent_id: ${client.get_agent_id() || "(awaiting server assignment)"}`);

  runModuleButton.addEventListener("click", async () => {
    if (!moduleHasRun) {
      moduleHasRun = true;
      medicalNoteEl?.setAttribute("hidden", "");
      // Locked, not hidden: the choice made for this run stays visible but can no longer be changed.
      if (uploadConsentCheckbox) uploadConsentCheckbox.disabled = true;
    }

    const selectedModule = WORKFLOW_MODULES.get(moduleSelect.value);
    runModuleButton.disabled = true;
    moduleSelect.disabled = true;
    runModuleButton.textContent = selectedModule ? `Running ${selectedModule.label}...` : "Running module...";

    try {
      await runSelectedWorkflowModule();
    } catch (error) {
      append(`${selectedModule?.label ?? "workflow"} module error: ${describeError(error)}`);
      console.error(error);
    } finally {
      runModuleButton.disabled = false;
      moduleSelect.disabled = false;
      runModuleButton.textContent = "Run";
    }
  });

  // Logged to the server tty (via the same "Client event from ..." line eye_detection events use) so a
  // checkbox toggle is independently verifiable server-side, regardless of whether any module is running.
  uploadConsentCheckbox?.addEventListener("change", () => {
    const checked = uploadConsentCheckbox.checked;
    append(`upload consent checkbox: ${checked ? "checked" : "unchecked"}`);
    client.send(
      JSON.stringify({
        type: "et-client-event",
        capability: "consent",
        action: "upload_consent_changed",
        details: { checked },
      }),
    );
  });

  window.client = client;
  window.sendAlive = () => client.send_alive();
} catch (error) {
  append(`error: ${error instanceof Error ? error.message : String(error)}`);
  console.error(error);
}
