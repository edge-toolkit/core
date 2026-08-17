// et_ws_pymath1.js -- Pyodide-based Python module shim
// Interface: default() (init), run()
//
// Storage-driven FedAvg: the shim owns the browser I/O (WebSocket, the math1-input pointer
// broadcast, storage GET/PUT); the wheel owns the kernel. The shim fetches the input JSON the
// pointer names, hands the raw text to Python, and stores the returned output JSON in this
// agent's bucket for the test harness to verify.

const PYODIDE_BASE_PATH = "/modules/pyodide/";

let pyodide = null;
let pyMod = null;

function loadPyodideScript() {
  return new Promise((resolve, reject) => {
    if (globalThis.loadPyodide) return resolve();
    const s = document.createElement("script");
    s.src = `${PYODIDE_BASE_PATH}pyodide.js`;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
  });
}

export default async function init() {
  await loadPyodideScript();
  // The full Pyodide distribution is served at /modules/pyodide/, so the runtime resolves from this
  // same origin -- no CDN dependency. pymath1 has no PyPI deps: its FedAvg kernel is stdlib-only, so
  // the only wheel to load is its own, served next to this shim.
  pyodide = await globalThis.loadPyodide({ indexURL: PYODIDE_BASE_PATH });

  const pkg = await fetch(new URL("package.json", import.meta.url)).then((r) => r.json());
  const wheelName = `${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  const bytes = new Uint8Array(await fetch(new URL(wheelName, import.meta.url)).then((r) => r.arrayBuffer()));
  pyodide.FS.writeFile(`/tmp/${wheelName}`, bytes);
  pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheelName}")`);

  // Start Pyodide coverage before importing so import-time lines count (no-op unless the runner set the gate).
  if (globalThis.__etPyCov) await globalThis.__etPyCov.start(pyodide, "pymath1");

  const pymath1 = pyodide.pyimport("pymath1");
  pyMod = {
    run: pymath1.run,
  };
}

export async function run() {
  if (!pyMod) throw new Error("pymath1: not initialized");

  const loc = typeof location !== "undefined" ? location : null;
  const wsProto = loc?.protocol === "https:" ? "wss:" : "ws:";
  const wsHost = loc?.host ?? "localhost:8080";
  const wsUrl = globalThis.__ET_WS_URL || `${wsProto}//${wsHost}/ws`;

  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  const client = new WsClient(new WsClientConfig(wsUrl));

  let pointer = null;
  client.set_on_message((frame) => {
    if (typeof frame !== "string") return;
    try {
      const msg = JSON.parse(frame);
      if (msg.type === "math1-input" && msg.bucket && msg.filename) pointer = msg;
    } catch {}
  });

  client.connect();

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const waitFor = async (what, ready) => {
    for (let i = 0; i < 100; i++) {
      const value = ready();
      if (value) return value;
      await sleep(100);
    }
    throw new Error(`Timeout waiting for ${what}`);
  };

  const log = (msg) => {
    console.log(msg);
    const el = document.getElementById("module-output");
    if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
  };

  try {
    await waitFor("WebSocket connection", () => client.get_state() === "connected");
    const agentId = await waitFor("agent_id", () => client.get_agent_id());

    log(`[pymath1] waiting for the math1-input pointer broadcast`);
    const inputPtr = await waitFor("math1-input pointer", () => pointer);
    log(`[pymath1] reading input from /storage/${inputPtr.bucket}/${inputPtr.filename}`);
    const inputResponse = await fetch(`/storage/${inputPtr.bucket}/${inputPtr.filename}`);
    if (!inputResponse.ok) throw new Error(`input GET failed: ${inputResponse.status}`);
    const inputJson = await inputResponse.text();

    const output = pyMod.run(agentId, inputJson, pyodide.toPy(log));
    const putResponse = await fetch(`/storage/${agentId}/math1-output.json`, { method: "PUT", body: output });
    if (!putResponse.ok) throw new Error(`output PUT failed: ${putResponse.status}`);
    log(`[pymath1] stored the global model to /storage/${agentId}/math1-output.json`);
    await sleep(2000);
  } catch (err) {
    log(`pymath1 run failed: ${String(err)}`);
    throw err;
  } finally {
    if (globalThis.__etPyCov) await globalThis.__etPyCov.stop(pyodide, "pymath1");
    client.disconnect();
  }
}
