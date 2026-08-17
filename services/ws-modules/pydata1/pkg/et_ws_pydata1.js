// et_ws_pydata1.js -- Pyodide-based Python module shim
// Interface: default(wasmUrl), metadata(), run()

const PYODIDE_BASE_PATH = "/modules/pyodide/";

let pyodide = null;
let pyMod = null;
let moduleVersion = null;

function loadPyodideScript() {
  return new Promise((resolve, reject) => {
    if (globalThis.loadPyodide) return resolve();
    // In Deno / non-browser environments, import() the module directly.
    if (
      typeof document === "undefined" ||
      typeof document.createElement !== "function" ||
      !document.head ||
      typeof document.head.appendChild !== "function" ||
      typeof Deno !== "undefined"
    ) {
      const baseUrl = typeof globalThis.__ET_HTTP_BASE === "string" ? globalThis.__ET_HTTP_BASE : "";
      const url = baseUrl + PYODIDE_CDN;
      import(url)
        .then((mod) => {
          if (mod.loadPyodide) globalThis.loadPyodide = mod.loadPyodide;
          resolve();
        })
        .catch(reject);
      return;
    }
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
  // and `micropip.install("httpx")` resolve from this same origin -- no CDN
  // dependency at runtime.
  pyodide = await globalThis.loadPyodide({ indexURL: PYODIDE_BASE_PATH });

  // pydata1's runtime stack: PyPI deps via micropip (httpx + attrs power
  // the generated client; pyodide-http rewires httpx to use the browser's
  // fetch()), plus two local wheels -- pydata1 itself (next to this shim)
  // and the generated et-rest-client wheel served by its own ws-module
  // mount at /modules/et-rest-client/. Going through micropip for the
  // local wheels would make it look up "et-rest-client" on PyPI, which we
  // deliberately don't publish. Pyodide unvendors `ssl` from the stdlib
  // (loaded on demand via loadPackage) and our generated httpx-based
  // client imports it at module top-level.
  await pyodide.loadPackage(["micropip", "ssl"]);
  const micropip = pyodide.pyimport("micropip");
  await micropip.install("httpx");
  await micropip.install("attrs");
  await micropip.install("pyodide-http");

  const { installWheel: installEtRestClient } = await import("/modules/et-rest-client/et_rest_client.js");
  await installEtRestClient(pyodide);

  const injectWheel = async (wheelName) => {
    const bytes = new Uint8Array(await fetch(new URL(wheelName, import.meta.url)).then((r) => r.arrayBuffer()));
    pyodide.FS.writeFile(`/tmp/${wheelName}`, bytes);
    pyodide.runPython(`import sys\nsys.path.insert(0, "/tmp/${wheelName}")`);
  };
  const pkg = await fetch(new URL("package.json", import.meta.url)).then((r) => r.json());
  moduleVersion = pkg.version;
  const ownWheel = `${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
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
