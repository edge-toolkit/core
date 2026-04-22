// et_ws_zig_data1.js — zig-data1 WASM module
// Runs WASM in a Web Worker; main thread proxies WebSocket + fetch via SharedArrayBuffer.
// Shared memory layout (Int32 offsets):
//   [0] signal: 0=idle, 1=request-pending
//   [1] request type: 0=sleep, 1=ws_connect, 2=ws_send, 3=ws_get_state, 4=ws_get_agent_id,
//                     5=ws_pop_response, 6=put_file, 7=get_file, 8=ws_disconnect,
//                     9=log, 10=set_status, 11=get_ws_url, 12=get_iso_timestamp
//   [2] payload length (also response length)
//   [3] aux length (for put_file body)
// Data area starts at byte offset 16.

export default async function init() {}

export async function run() {
  const DATA_OFFSET = 16;
  const sab = new SharedArrayBuffer(64 * 1024);
  const ctrl = new Int32Array(sab, 0, 4);
  const data = new Uint8Array(sab, DATA_OFFSET);
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  const wasmUrl = new URL("et_ws_zig_data1.wasm", import.meta.url).href;
  const workerUrl = new URL("et_ws_zig_data1_worker.js", import.meta.url).href;

  const respond = (str = "") => {
    if (str) {
      const b = enc.encode(str);
      data.set(b);
      Atomics.store(ctrl, 2, b.length);
    } else Atomics.store(ctrl, 2, 0);
    Atomics.store(ctrl, 0, 0);
    Atomics.notify(ctrl, 0);
  };

  return new Promise((resolve, reject) => {
    let ws = null, wsState = "disconnected", agentId = "", lastResponse = null;

    const poll = () => {
      if (Atomics.load(ctrl, 0) !== 1) {
        setTimeout(poll, 0);
        return;
      }

      const type = Atomics.load(ctrl, 1);
      const plen = Atomics.load(ctrl, 2);
      const alen = Atomics.load(ctrl, 3);
      const copy = (off, len) => dec.decode(Uint8Array.from(data.subarray(off, off + len)));
      const payload = copy(0, plen);
      const aux = alen ? copy(plen, alen) : "";

      switch (type) {
        case 0:
          setTimeout(() => {
            respond();
            poll();
          }, parseInt(payload) || 0);
          return;
        case 1:
          ws = new WebSocket(payload);
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
          respond();
          break;
        case 2:
          ws?.send(payload);
          respond();
          break;
        case 3:
          respond(wsState);
          break;
        case 4:
          respond(agentId);
          break;
        case 5: {
          const r = lastResponse ?? "";
          lastResponse = null;
          respond(r);
          break;
        }
        case 6:
          fetch(payload, { method: "PUT", body: aux })
            .then(() => {
              respond();
              poll();
            }).catch(() => {
              respond();
              poll();
            });
          return;
        case 7:
          fetch(payload).then(r => r.text())
            .then(t => {
              respond(t);
              poll();
            }).catch(() => {
              respond();
              poll();
            });
          return;
        case 8:
          ws?.close();
          wsState = "disconnected";
          respond();
          break;
        case 9:
          console.log(payload);
          appendOutput(payload);
          respond();
          break;
        case 10:
          appendOutput(payload);
          respond();
          break;
        case 11: {
          const p = location.protocol === "https:" ? "wss:" : "ws:";
          respond(`${p}//${location.host}/ws`);
          break;
        }
        case 12:
          respond(new Date().toISOString());
          break;
        default:
          respond();
          break;
      }
      setTimeout(poll, 0);
    };

    const worker = new Worker(workerUrl, { type: "classic" });
    worker.onmessage = (e) => {
      if (e.data.done) {
        worker.terminate();
        e.data.ret === 0 ? resolve() : reject(new Error("zig-data1: run() returned " + e.data.ret));
      }
    };
    worker.onerror = (e) => {
      worker.terminate();
      reject(e);
    };
    worker.postMessage({ sab, wasmUrl });
    poll();
  });
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
