// et_ws_dotnet_data1.js — .NET WASM shim for dotnet-data1
// Interface: default(), run()

let exports = null;

export default async function init() {
  const { dotnet } = await import(new URL("dotnet.js", import.meta.url).href);
  const { getAssemblyExports, setModuleImports } = await dotnet.create();

  let ws = null, wsState = "disconnected", agentId = "", lastResponse = null;

  setModuleImports("dotnet-data1", {
    wsConnect: (url) => {
      ws = new WebSocket(url);
      wsState = "connecting";
      ws.onopen = () => {
        wsState = "connected";
        ws.send(JSON.stringify({ type: "connect" }));
      };
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (msg.type === "connect_ack" && msg.agent_id) agentId = msg.agent_id;
          else if (msg.type === "response" && msg.message) lastResponse = msg.message;
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
    wsSend: (msg) => ws?.send(msg),
    wsGetState: () => wsState,
    wsGetAgentId: () => agentId ?? "",
    wsPopResponse: () => {
      const r = lastResponse ?? "";
      lastResponse = null;
      return r;
    },
    putFile: (url, body) =>
      fetch(url, { method: "PUT", body }).then(r => {
        if (!r.ok) throw new Error(`PUT failed: ${r.status}`);
      }),
    getFile: (url) =>
      fetch(url).then(r => {
        if (!r.ok) throw new Error(`GET failed: ${r.status}`);
        return r.text();
      }),
    log: (msg) => {
      console.log(msg);
      appendOutput(msg);
    },
    setStatus: (msg) => appendOutput(msg),
    getWsUrl: () => `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`,
    getIsoTimestamp: () => new Date().toISOString(),
    sleep: (ms) => new Promise(r => setTimeout(r, ms)),
  });

  exports = await getAssemblyExports("dotnet-data1");
}

export async function run() {
  if (!exports) throw new Error("dotnet-data1: not initialized");
  await exports.DotnetData1.Run();
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
