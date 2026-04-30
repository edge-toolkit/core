import init, { initTracing, WsClient, WsClientConfig } from "/modules/et-ws-wasm-agent/et_ws_wasm_agent.js";

console.log("app.js: module loading started");

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

const STORED_AGENT_ID_KEY = "et_ws_wasm_agent.agent_id";
let currentAgentId = null;

const append = (line) => {
  logEl.textContent += `\n${line}`;
};

const describeError = (error) => (
  error instanceof Error ? error.message : String(error)
);

const WORKFLOW_MODULES = new Map();

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
      if (name === "onnxruntime-web" || name === "pyodide") {
        append(`Skipping ${name}: already loaded as a dependency`);
        continue;
      }
      const pkgResp = await fetch(`/modules/${name}/package.json`);
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

  const loadedModule = await loadWorkflowModule(moduleKey);
  if (
    typeof loadedModule.is_running === "function"
    && loadedModule.is_running()
    && typeof loadedModule.stop === "function"
  ) {
    append(`${moduleConfig.label} module: calling stop()`);
    loadedModule.stop();
    append(`${moduleConfig.label} module stopped`);
    return;
  }

  append(`${moduleConfig.label} module: calling run()`);
  const runPromise = loadedModule.run();
  append(`${moduleConfig.label} module: run() started`);
  await runPromise;
  append(`${moduleConfig.label} module run() returned`);
};

const handleProtocolMessage = (message) => {
  let parsed;

  try {
    parsed = JSON.parse(message);
  } catch {
    return;
  }

  if (parsed?.type !== "connect_ack" || typeof parsed.agent_id !== "string") {
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

logEl.textContent =
  `Initializing WASM from /modules/et-ws-wasm-agent/et_ws_wasm_agent_bg.wasm\nWebSocket endpoint: ${wsUrl}`;
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
      updateAgentCard("Connecting to websocket server...", client.get_client_id() || readStoredAgentId());
    } else if (state === "connected") {
      updateAgentCard(
        "Socket connected. Waiting for server identity acknowledgement...",
        client.get_client_id() || readStoredAgentId(),
      );
    } else if (state === "reconnecting") {
      updateAgentCard(
        "Disconnected. Trying to re-use retained agent ID...",
        client.get_client_id() || readStoredAgentId(),
      );
    } else if (state === "disconnected") {
      updateAgentCard(
        "Socket disconnected. Retained agent ID will be re-used on next connect.",
        client.get_client_id() || readStoredAgentId(),
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
    client.get_client_id() || retainedAgentId,
  );
  append(`client_id: ${client.get_client_id() || "(awaiting server assignment)"}`);

  runModuleButton.addEventListener("click", async () => {
    const selectedModule = WORKFLOW_MODULES.get(moduleSelect.value);
    runModuleButton.disabled = true;
    moduleSelect.disabled = true;
    runModuleButton.textContent = selectedModule
      ? `Running ${selectedModule.label}...`
      : "Running module...";

    try {
      await runSelectedWorkflowModule();
    } catch (error) {
      append(`${selectedModule?.label ?? "workflow"} module error: ${describeError(error)}`);
      console.error(error);
    } finally {
      runModuleButton.disabled = false;
      moduleSelect.disabled = false;
      runModuleButton.textContent = "Run module";
    }
  });

  window.client = client;
  window.sendAlive = () => client.send_alive();
} catch (error) {
  append(`error: ${error instanceof Error ? error.message : String(error)}`);
  console.error(error);
}
