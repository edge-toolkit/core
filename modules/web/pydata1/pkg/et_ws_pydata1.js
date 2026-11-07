// et_ws_pydata1.js -- Pyodide-based Python module shim
// Interface: default(wasmUrl), metadata(), run()

let pyodide = null;
let pyMod = null;
let moduleVersion = null;

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export default async function init() {
  const { startPyodide } = await import("/modules/@edge-toolkit/edge-toolkit/edge_toolkit.js");
  // `mise install pyodide` extracts the full GitHub-release distribution (~200 MB of pinned wheels) at
  // /modules/pyodide/, so `micropip.install("httpx")` resolves from the same origin as the runtime.
  pyodide = await startPyodide();

  // pydata1's runtime stack: PyPI deps via micropip (httpx + attrs power
  // the generated client; pyodide-http rewires httpx to use the browser's
  // fetch()), plus two local wheels -- pydata1 itself (next to this shim)
  // and the generated et-rest-client wheel served by its own ws-module
  // mount at /modules/@edge-toolkit/et-rest-client/. Going through micropip for the
  // local wheels would make it look up "et-rest-client" on PyPI, which we
  // deliberately don't publish. Pyodide unvendors `ssl` from the stdlib
  // (loaded on demand via loadPackage) and our generated httpx-based
  // client imports it at module top-level.
  await pyodide.loadPackage(["micropip", "ssl"]);
  const micropip = pyodide.pyimport("micropip");
  await micropip.install("httpx");
  await micropip.install("attrs");
  await micropip.install("pyodide-http");

  const { installWheel: installEtRestClient } = await import("/modules/@edge-toolkit/et-rest-client/et_rest_client.js");
  await installEtRestClient(pyodide);

  const injectWheel = async (wheelName) => {
    const response = await fetch(new URL(wheelName, import.meta.url));
    const bytes = new Uint8Array(await response.arrayBuffer());
    pyodide.FS.writeFile(`/tmp/${wheelName}`, bytes);
    pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheelName}")`);
  };
  const pkg = await fetch(new URL("package.json", import.meta.url)).then((response) => response.json());
  moduleVersion = pkg.version;
  const distribution = pkg.name.split("/").pop();
  const ownWheel = `${distribution.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  await injectWheel(ownWheel);

  // Start Pyodide coverage before importing so import-time lines count (no-op unless the runner set the gate).
  if (globalThis.__etPyCov) await globalThis.__etPyCov.start(pyodide, "pydata1");

  const pydata1 = pyodide.pyimport("pydata1");
  pyMod = {
    run: pydata1.run,
  };
}

export async function run() {
  if (!pyMod) throw new Error("pydata1: not initialized");

  const loc = typeof location !== "undefined" ? location : null;
  const wsProto = loc?.protocol === "https:" ? "wss:" : "ws:";
  const wsHost = loc?.host ?? "localhost:8080";
  const wsUrl = globalThis.__ET_WS_URL || `${wsProto}//${wsHost}/ws`;

  const wasmAgent = await import("/modules/@edge-toolkit/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  const client = new WsClient(new WsClientConfig(wsUrl));

  client.connect();

  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

  for (let attempt = 0; attempt < 100; attempt++) {
    if (client.get_state() === "connected") break;
    await sleep(100);
    if (attempt === 99) throw new Error("Timeout waiting for WebSocket connection");
  }

  let agentId = "";
  for (let attempt = 0; attempt < 100; attempt++) {
    agentId = client.get_agent_id();
    if (agentId) break;
    await sleep(100);
    if (attempt === 99) throw new Error("Timeout waiting for agent_id");
  }

  const log = (msg) => {
    console.log(msg);
    const el = document.getElementById("module-output");
    if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
  };
  // Diagnostic marker: this is the version fetched from package.json + injected as the wheel filename, so
  // seeing the right number here after a version bump proves the module reloaded fresh (no stale-cache reuse
  // of the package.json/wheel fetches below, neither of which is cache-busted).
  log(`pydata1 version: ${moduleVersion}`);

  // The Python side runs `Client(base_url=...)` against this origin and
  // does PUT/GET itself via the generated client + pyodide-http patch.
  try {
    await pyMod.run(
      agentId,
      window.location.origin,
      pyodide.toPy(sleep),
      pyodide.toPy(log),
      pyodide.toPy(() => {}),
      pyodide.toPy(() => document.getElementById("upload-consent")?.checked ?? false),
    );
  } catch (err) {
    log(`pydata1 run failed: ${String(err)}`);
    throw err;
  } finally {
    if (globalThis.__etPyCov) await globalThis.__etPyCov.stop(pyodide, "pydata1");
    client.disconnect();
  }
}
