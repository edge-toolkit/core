// et_ws_zig_math1.js — zig-math1 WASM module
// Runs WASM in a Web Worker; main thread proxies WebSocket + REST calls via
// SharedArrayBuffer. Shared memory layout (Int32 offsets):
//   [0] signal: 0=idle, 1=request-pending
//   [1] request type: 0=sleep, 1=ws_connect, 2=ws_get_state, 3=ws_get_agent_id,
//                     4=ws_get_input, 6=ws_disconnect, 7=log, 8=set_status,
//                     9=get_ws_url, 11=rest_request
//   [2] payload length (also response length; -1 on rest_request failure)
//   [3] aux length (binary request body for rest_request)
// Data area starts at byte offset 16.

export default async function init() {}

export async function run() {
  const DATA_OFFSET = 16;
  const sab = new SharedArrayBuffer(64 * 1024);
  const ctrl = new Int32Array(sab, 0, 4);
  const data = new Uint8Array(sab, DATA_OFFSET);
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  const workerUrl = new URL("et_ws_zig_math1_worker.js", import.meta.url).href;

  const respond = (str = "") => {
    if (str) {
      const b = enc.encode(str);
      data.set(b);
      Atomics.store(ctrl, 2, b.length);
    } else Atomics.store(ctrl, 2, 0);
    Atomics.store(ctrl, 0, 0);
    Atomics.notify(ctrl, 0);
  };

  const respondBytes = (bytes) => {
    data.set(bytes);
    Atomics.store(ctrl, 2, bytes.length);
    Atomics.store(ctrl, 0, 0);
    Atomics.notify(ctrl, 0);
  };

  const respondError = () => {
    Atomics.store(ctrl, 2, -1);
    Atomics.store(ctrl, 0, 0);
    Atomics.notify(ctrl, 0);
  };

  return new Promise((resolve, reject) => {
    let ws = null,
      wsState = "disconnected",
      agentId = "",
      inputPointer = null;

    const poll = () => {
      if (Atomics.load(ctrl, 0) !== 1) {
        setTimeout(poll, 0);
        return;
      }

      const type = Atomics.load(ctrl, 1);
      const plen = Atomics.load(ctrl, 2);
      const alen = Atomics.load(ctrl, 3);
      const payload = dec.decode(Uint8Array.from(data.subarray(0, plen)));

      switch (type) {
        case 0:
          setTimeout(
            () => {
              respond();
              poll();
            },
            parseInt(payload) || 0,
          );
          return;
        case 1:
          ws = new WebSocket(payload);
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
          respond();
          break;
        case 2:
          respond(wsState);
          break;
        case 3:
          respond(agentId);
          break;
        case 4:
          respond(inputPointer ? `${inputPointer.bucket}\n${inputPointer.filename}` : "");
          break;
        case 6:
          ws?.close();
          wsState = "disconnected";
          respond();
          break;
        case 7:
          console.log(payload);
          appendOutput(payload);
          respond();
          break;
        case 8:
          appendOutput(payload);
          respond();
          break;
        case 9: {
          const p = location.protocol === "https:" ? "wss:" : "ws:";
          respond(`${p}//${location.host}/ws`);
          break;
        }
        case 11: {
          // payload = "METHOD url", aux = binary body. Response is the raw
          // body bytes; signal failures with respondError() so the Zig
          // extern returns -1.
          const spaceIdx = payload.indexOf(" ");
          const method = payload.substring(0, spaceIdx);
          const url = payload.substring(spaceIdx + 1);
          const opts = { method };
          if (alen > 0) {
            opts.body = new Uint8Array(data.subarray(plen, plen + alen)).slice();
          }
          fetch(url, opts)
            .then((r) => {
              if (!r.ok) throw new Error(`HTTP ${r.status}`);
              return r.arrayBuffer();
            })
            .then((buf) => {
              respondBytes(new Uint8Array(buf));
              poll();
            })
            .catch(() => {
              respondError();
              poll();
            });
          return;
        }
        default:
          respond();
          break;
      }
      setTimeout(poll, 0);
    };

    const worker = new Worker(workerUrl, { type: "module" });
    worker.onmessage = (e) => {
      if (e.data.done) {
        worker.terminate();
        if (e.data.ret === 0) {
          resolve();
        } else {
          reject(new Error("zig-math1: run() returned " + e.data.ret));
        }
      }
    };
    worker.onerror = (e) => {
      worker.terminate();
      reject(e);
    };
    // The worker resolves its own wasm URL from import.meta.url; only the shared buffer crosses the boundary.
    worker.postMessage({ sab });
    poll();
  });
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}
