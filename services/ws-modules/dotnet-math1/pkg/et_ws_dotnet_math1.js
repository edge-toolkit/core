// et_ws_dotnet_math1.js — .NET WASM shim for dotnet-math1
// Interface: default(), run()
//
// Storage-driven FedAvg: the shim owns the browser I/O -- the WebSocket (including capturing the
// broadcast math1-input pointer), fetching the input JSON from storage, and storing the output --
// while the C# guest parses the JSON with System.Text.Json and owns the kernel.

let exports = null;

// skipcq: JS-0833 -- committed .NET WASM ES-module shim; DeepSource's script-mode parse is a false positive
export default async function init() {
  const { dotnet } = await import(new URL("dotnet.js", import.meta.url).href);
  const { getAssemblyExports, setModuleImports } = await dotnet.create();

  let ws = null,
    wsState = "disconnected",
    agentId = "",
    inputPointer = null;

  setModuleImports("dotnet-math1", {
    wsConnect: (url) => {
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
          if (msg.type === "math1-input" && msg.bucket && msg.filename) inputPointer = msg;
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
    hasInput: () => inputPointer !== null,
    fetchInputJson: () =>
      fetch(`/storage/${inputPointer.bucket}/${inputPointer.filename}`).then((r) => {
        if (!r.ok) throw new Error(`input GET failed: ${r.status}`);
        return r.text();
      }),
    putOutput: (module, weight, bias) => {
      const body = JSON.stringify({ module, weight, bias });
      return fetch(`/storage/${agentId}/math1-output.json`, { method: "PUT", body }).then((r) => {
        if (!r.ok) throw new Error(`output PUT failed: ${r.status}`);
      });
    },
    log: (msg) => {
      console.log(msg);
      appendOutput(msg);
    },
    setStatus: (msg) => appendOutput(msg),
    getWsUrl: () => `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`,
    sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  });

  exports = await getAssemblyExports("dotnet-math1");
}

export async function run() {
  if (!exports) throw new Error("dotnet-math1: not initialized");
  await exports.EtWsModules.DotnetMath1.RunAsync();
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
