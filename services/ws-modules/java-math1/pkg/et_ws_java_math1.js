// et_ws_java_math1.js -- TeaVM JS shim for java-math1
// Interface: default(), run()
//
// Storage-driven FedAvg: the shim owns the browser I/O -- the WebSocket (including capturing the
// broadcast math1-input pointer), fetching the input JSON from storage, and storing the output --
// while the TeaVM guest owns the kernel, reading the parsed input through the typed host accessors
// below (the guest carries no JSON parser of its own; the accessors keep it dependency-free).

let javaRun = null;

export default async function init() {
  let ws = null,
    wsState = "disconnected",
    agentId = "",
    inputPointer = null,
    input = null;

  // Ack/broadcast values feed the /storage/ fetch URLs below; accept only single, traversal-free
  // path segments so a hostile frame cannot steer those requests.
  const SAFE_SEGMENT = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

  // TeaVM @JSBody calls reference `host` as a global
  globalThis.host = {
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
          if (msg.type === "et-connect-ack" && SAFE_SEGMENT.test(msg.agent_id)) agentId = msg.agent_id;
          const bucket = msg.bucket;
          const filename = msg.filename;
          if (msg.type === "math1-input" && SAFE_SEGMENT.test(bucket) && SAFE_SEGMENT.test(filename)) {
            inputPointer = { bucket, filename };
          }
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
    loadInput: () =>
      fetch(`/storage/${inputPointer.bucket}/${inputPointer.filename}`).then((r) => {
        if (!r.ok) throw new Error(`input GET failed: ${r.status}`);
        return r.json().then((json) => {
          input = json;
        });
      }),
    inputClientCount: () => input.clients.length,
    inputSampleCount: (client) => input.clients[client].length,
    inputFeature: (client, index) => input.clients[client][index][0],
    inputTarget: (client, index) => input.clients[client][index][1],
    inputRounds: () => input.rounds,
    inputEpochs: () => input.epochs,
    inputLearningRate: () => input.learning_rate,
    inputDescribe: () => `${input.clients.length} clients x ${input.rounds} rounds x ${input.epochs} local epochs`,
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
  if (!javaRun) throw new Error("java-math1: not initialized");
  await javaRun();
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
