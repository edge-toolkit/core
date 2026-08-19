// et_ws_kotlin_data1.js -- ES module shim for kotlin-data1 (Kotlin/Wasm, WasmGC)
// Interface: default(), run()

export default async function init() {
  let ws = null,
    wsState = "disconnected",
    agentId = "";

  // The Kotlin js() interop bridges reference `host` as a global
  globalThis.host = {
    wsConnect: (url) => {
      // Cleared per connect: the page caches this module, so a second run() must not see the first run's id.
      agentId = "";
      ws = new WebSocket(url);
      wsState = "connecting";
      ws.onopen = () => {
        wsState = "connected";
        ws.send(JSON.stringify({ type: "et-connect" }));
      };
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (msg.type === "et-connect-ack" && msg.agent_id) agentId = msg.agent_id;
        } catch {}
      };
      ws.onclose = ws.onerror = () => {
        wsState = "disconnected";
      };
    },
    wsDisconnect: () => {
      ws?.close();
      wsState = "disconnected";
    },
    wsGetState: () => wsState,
    wsGetAgentId: () => agentId ?? "",
    putFile: (url, body) =>
      fetch(url, { method: "PUT", body }).then((r) => {
        if (!r.ok) throw new Error(`PUT failed: ${r.status}`);
      }),
    getFile: (url) =>
      fetch(url).then((r) => {
        if (!r.ok) throw new Error(`GET failed: ${r.status}`);
        return r.text();
      }),
    log: (msg) => {
      console.log(msg);
      appendOutput(msg);
    },
    setStatus: (msg) => appendOutput(msg),
    getWsUrl: () => `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`,
    sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  };

  // Instantiates the WasmGC module (browser path: instantiateStreaming over fetch) and runs Kotlin main(),
  // which installs globalThis.kotlinData1Run.
  await import(new URL("et_ws_kotlin_data1_compiled.mjs", import.meta.url).href);
}

export async function run() {
  if (typeof globalThis.kotlinData1Run !== "function") {
    throw new Error("kotlin-data1: not initialized");
  }
  await globalThis.kotlinData1Run();
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
