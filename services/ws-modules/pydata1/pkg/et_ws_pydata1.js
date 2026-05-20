// et_ws_pydata1.js — Pyodide-based Python module shim
// Interface: default(wasmUrl), metadata(), run()

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
  // `mise install pyodide` extracts the full GitHub-release distribution
  // (~200 MB of pinned wheels) at /modules/pyodide/, so both the runtime
  // and `micropip.install("httpx")` resolve from this same origin — no CDN
  // dependency at runtime.
  pyodide = await globalThis.loadPyodide({ indexURL: PYODIDE_BASE_PATH });

  // pydata1's runtime stack is split between PyPI deps (httpx + attrs power
  // the generated client; pyodide-http rewires httpx to use the browser's
  // fetch()) and two local wheels: the generated et-rest-client and pydata1
  // itself. We bring the PyPI deps in via micropip (which resolves
  // transitively), then sys.path-inject the local wheels — same pattern as
  // pyface1. Going through micropip for the local wheels would make it look
  // up "et-rest-client" on PyPI, which we deliberately don't publish.
  // Pyodide unvendors `ssl` from the stdlib (loaded on demand via loadPackage)
  // and our generated httpx-based client imports it at module top-level.
  await pyodide.loadPackage(["micropip", "ssl"]);
  const micropip = pyodide.pyimport("micropip");
  await micropip.install("httpx");
  await micropip.install("attrs");
  await micropip.install("pyodide-http");

  const injectWheel = async (wheelName) => {
    const bytes = new Uint8Array(await fetch(new URL(wheelName, import.meta.url)).then(r => r.arrayBuffer()));
    pyodide.FS.writeFile(`/tmp/${wheelName}`, bytes);
    pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheelName}")`);
  };
  const pkg = await fetch(new URL("package.json", import.meta.url)).then(r => r.json());
  const ownWheel = `${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  await injectWheel("et_rest_client-0.1.0-py3-none-any.whl");
  await injectWheel(ownWheel);

  const pydata1 = pyodide.pyimport("pydata1");
  pyMod = {
    run: pydata1.run,
  };
}

export async function run() {
  if (!pyMod) throw new Error("pydata1: not initialized");

  const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const wsUrl = `${wsProtocol}//${window.location.host}/ws`;

  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  const client = new WsClient(new WsClientConfig(wsUrl));

  client.connect();

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  for (let i = 0; i < 100; i++) {
    if (client.get_state() === "connected") break;
    await sleep(100);
    if (i === 99) throw new Error("Timeout waiting for WebSocket connection");
  }

  let agentId = "";
  for (let i = 0; i < 100; i++) {
    agentId = client.get_agent_id();
    if (agentId) break;
    await sleep(100);
    if (i === 99) throw new Error("Timeout waiting for agent_id");
  }

  const log = (msg) => {
    console.log(msg);
    const el = document.getElementById("module-output");
    if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
  };

  // The Python side runs `Client(base_url=...)` against this origin and
  // does PUT/GET itself via the generated client + pyodide-http patch.
  try {
    await pyMod.run(
      agentId,
      window.location.origin,
      pyodide.toPy(sleep),
      pyodide.toPy(log),
      pyodide.toPy(() => {}),
    );
  } finally {
    client.disconnect();
  }
}
