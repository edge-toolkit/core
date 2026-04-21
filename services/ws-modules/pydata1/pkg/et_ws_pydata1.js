// et_ws_pydata1.js — Pyodide-based Python module shim
// Interface: default(wasmUrl), metadata(), run()

const PYODIDE_CDN = "https://cdn.jsdelivr.net/pyodide/v0.29.3/full/pyodide.js";

let pyodide = null;
let pyMod = null;

function loadPyodideScript() {
  return new Promise((resolve, reject) => {
    if (globalThis.loadPyodide) return resolve();
    const s = document.createElement("script");
    s.src = PYODIDE_CDN;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
  });
}

export default async function init() {
  await loadPyodideScript();
  pyodide = await globalThis.loadPyodide();

  const pkgUrl = new URL("package.json", import.meta.url);
  const pkg = await fetch(pkgUrl).then(r => r.json());
  const wheelName = `${pkg.name.replace(/-/g, "_")}-${pkg.version}-py3-none-any.whl`;
  const wheelUrl = new URL(`${wheelName}`, import.meta.url);

  await pyodide.loadPackage("micropip");
  const micropip = pyodide.pyimport("micropip");
  await micropip.install(wheelUrl.href);
  const pydata1 = pyodide.pyimport("pydata1");
  pyMod = {
    run: pydata1.run,
  };
}

export async function run() {
  if (!pyMod) throw new Error("pydata1: not initialized");

  const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const wsUrl = `${wsProtocol}//${window.location.host}/ws`;

  const { WsClient, WsClientConfig } = await import("/pkg/et_ws_wasm_agent.js");
  const client = new WsClient(new WsClientConfig(wsUrl));

  let responseResolvers = [];
  client.set_on_message((raw) => {
    try {
      const msg = JSON.parse(raw);
      if (msg.type === "response") {
        for (const { prefix, resolve } of responseResolvers) {
          if (msg.message.startsWith(prefix)) {
            responseResolvers = responseResolvers.filter(r => r.resolve !== resolve);
            resolve(msg.message);
            return;
          }
        }
      }
    } catch { /* ignore */ }
  });

  client.connect();

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  for (let i = 0; i < 100; i++) {
    if (client.get_state() === "connected") break;
    await sleep(100);
    if (i === 99) throw new Error("Timeout waiting for WebSocket connection");
  }

  let agentId = "";
  for (let i = 0; i < 100; i++) {
    agentId = client.get_client_id();
    if (agentId) break;
    await sleep(100);
    if (i === 99) throw new Error("Timeout waiting for agent_id");
  }

  const wsSend = (msgStr) => {
    // inject agent_id into fetch_file messages
    const msg = JSON.parse(msgStr);
    if (msg.type === "fetch_file") msg.agent_id = agentId;
    client.send(JSON.stringify(msg));
  };

  const waitForResponse = (prefix) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        responseResolvers = responseResolvers.filter(r => r.resolve !== resolve);
        reject(new Error(`Timeout waiting for response with prefix: ${prefix}`));
      }, 5000);
      responseResolvers.push({
        prefix,
        resolve: (val) => {
          clearTimeout(timer);
          resolve(val);
        },
      });
    });

  const putFile = async (url, content) => {
    const resp = await fetch(url, { method: "PUT", mode: "cors", body: content });
    if (!resp.ok) throw new Error(`PUT failed: ${resp.status}`);
  };

  const getFile = async (url) => {
    const resp = await fetch(url);
    if (!resp.ok) throw new Error(`GET failed: ${resp.status}`);
    return resp.text();
  };

  const log = (msg) => {
    console.log(msg);
    const el = document.getElementById("module-output");
    if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
  };

  try {
    await pyMod.run(
      pyodide.toPy(wsSend),
      pyodide.toPy(waitForResponse),
      pyodide.toPy(putFile),
      pyodide.toPy(getFile),
      pyodide.toPy(sleep),
      pyodide.toPy(log),
      pyodide.toPy(() => {}),
    );
  } finally {
    client.disconnect();
  }
}
