// et_ws_java_data1.js -- TeaVM JS shim for java-data1
// Interface: default(), run()

let javaRun = null;

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export default async function init() {
  let ws = null,
    wsState = "disconnected",
    agentId = "";

  // TeaVM @JSBody calls reference `host` as a global
  globalThis.host = {
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
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  };

  const jsUrl = new URL("classes.js", import.meta.url).href;

  await new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = jsUrl;
    script.onload = resolve;
    script.onerror = reject;
    document.head.appendChild(script);
  });

  javaRun = globalThis.run;
}

export async function run() {
  if (!javaRun) throw new Error("java-data1: not initialized");
  await javaRun();
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
