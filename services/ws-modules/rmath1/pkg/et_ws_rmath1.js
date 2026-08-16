// et_ws_rmath1.js -- bootstrap only. The control loop is module.R's run(); this shim boots webR, sets up the
// agent WebSocket transport that the R code drives via webr::eval_js, and hands control to run().
//
// webR cannot open the agent WebSocket itself, so the transport (the shared et-ws-wasm-agent WsClient) lives
// here and is exposed on globalThis.__etAgent for R to drive; the broadcast math1-input pointer is captured
// onto __etAgent.input for R to poll. Everything else -- sequencing, the storage reads/writes (httr2 over the
// /websockify relay), the FedAvg kernel -- happens in module.R. webR is vendored under pkg/webr/ (see
// build-ws-rmath1-module) and served at the path below.

const WEBR_BASE_URL = "/modules/et-ws-rmath1/webr/";
const R_SOURCE_URL = "/modules/et-ws-rmath1/module.R";

let webR = null;

export default async function init() {
  const { WebR } = await import(`${WEBR_BASE_URL}webr.mjs`);
  webR = new WebR({ baseUrl: WEBR_BASE_URL });
  await webR.init();
  // Cache-bust so edits to module.R are picked up on reload.
  const rSource = await (await fetch(`${R_SOURCE_URL}?v=${Date.now()}`)).text();
  await webR.evalRVoid(rSource);
}

export async function run() {
  if (!webR) throw new Error("rmath1: not initialized");
  await setupAgent();
  // Hand control to R -- run() is the control loop.
  await webR.evalRVoid("run()");
}

// Expose the agent WebSocket to R on globalThis.__etAgent. R drives it (connect state, agent_id, disconnect)
// via webr::eval_js; the shim only creates and connects it, and captures the math1-input pointer broadcast.
async function setupAgent() {
  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  const loc = typeof location !== "undefined" ? location : null;
  const wsProto = loc?.protocol === "https:" ? "wss:" : "ws:";
  const wsHost = loc?.host ?? "localhost:8080";
  const wsUrl = globalThis.__ET_WS_URL || `${wsProto}//${wsHost}/ws`;
  const client = new WsClient(new WsClientConfig(wsUrl));
  globalThis.__etAgent = {
    client,
    input: null,
    log(msg) {
      console.log(`[rmath1] ${msg}`);
      const el = typeof document !== "undefined" ? document.getElementById("module-output") : null;
      if (el) el.value = (el.value ? `${el.value}\n` : "") + msg;
    },
  };
  client.set_on_message((frame) => {
    if (typeof frame !== "string") return;
    try {
      const msg = JSON.parse(frame);
      if (msg.type === "math1-input" && msg.bucket && msg.filename) globalThis.__etAgent.input = msg;
    } catch {}
  });
  client.connect();
}
