// et_ws_dotnet_data1.js -- .NET WASM shim for dotnet-data1
// Interface: default(), run()

let exports = null;

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export default async function init() {
  const { dotnet } = await import(new URL("dotnet.js", import.meta.url).href);
  const { getAssemblyExports, setModuleImports } = await dotnet.create();

  let ws = null,
    wsState = "disconnected",
    agentId = "";

  setModuleImports("dotnet-data1", {
    wsConnect: (url) => {
      ws = new WebSocket(url);
      wsState = "connecting";
      ws.onopen = () => {
        wsState = "connected";
        ws.send(JSON.stringify({ type: "et-connect" }));
      };
      ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data);
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
      fetch(url, { method: "PUT", body }).then((response) => {
        if (!response.ok) throw new Error(`PUT failed: ${response.status}`);
      }),
    getFile: (url) =>
      fetch(url).then((response) => {
        if (!response.ok) throw new Error(`GET failed: ${response.status}`);
        return response.text();
      }),
    log: (msg) => {
      console.log(msg);
      appendOutput(msg);
    },
    setStatus: (msg) => appendOutput(msg),
    getWsUrl: () => `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`,
    getIsoTimestamp: () => new Date().toISOString(),
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  });

  exports = await getAssemblyExports("dotnet-data1");
}

export async function run() {
  if (!exports) throw new Error("dotnet-data1: not initialized");
  await exports.EtWsModules.DotnetData1.RunAsync();
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
