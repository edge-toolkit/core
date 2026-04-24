// et_ws_java_data1.js — TeaVM JS shim for java-data1
// Interface: default(), run()

let javaRun = null;

export default async function init() {
  let ws = null, wsState = "disconnected", agentId = "", lastResponse = null;

  // TeaVM @JSBody calls reference `host` as a global
  globalThis.host = {
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
    sleep: (ms) => new Promise(r => setTimeout(r, ms)),
  };

  const jsUrl = new URL("classes.js", import.meta.url).href;

  await new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = jsUrl;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
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
