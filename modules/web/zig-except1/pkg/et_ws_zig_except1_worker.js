// et_ws_zig_except1_worker.js -- Web Worker for zig-except1 WASM module
const DATA_OFFSET = 16;
let ctrl, data, wasmMemory;
const enc = new TextEncoder(),
  dec = new TextDecoder();
const readStr = (ptr, len) => dec.decode(new Uint8Array(wasmMemory.buffer, ptr, len));

// String payload, response is a UTF-8 string.
function call(type, payload = "") {
  const pb = enc.encode(payload);
  data.set(pb);
  Atomics.store(ctrl, 3, 0);
  Atomics.store(ctrl, 2, pb.length);
  Atomics.store(ctrl, 1, type);
  Atomics.store(ctrl, 0, 1);
  Atomics.notify(ctrl, 0);
  Atomics.wait(ctrl, 0, 1); // block until main thread responds
  const rlen = Atomics.load(ctrl, 2);
  return dec.decode(Uint8Array.from(data.subarray(0, rlen)));
}

const writeBack = (r, buf, max) => {
  const b = enc.encode(r);
  const n = Math.min(b.length, max);
  new Uint8Array(wasmMemory.buffer, buf, n).set(b.subarray(0, n));
  return n;
};

const imports = {
  env: {
    js_log: (p, l) => call(7, readStr(p, l)),
    js_set_status: (p, l) => call(8, readStr(p, l)),
    js_ws_connect: (p, l) => call(1, readStr(p, l)),
    js_ws_disconnect: () => call(6),
    js_ws_get_state: (buf, max) => writeBack(call(2), buf, max),
    js_ws_get_agent_id: (buf, max) => writeBack(call(3), buf, max),
    js_sleep_ms: (ms) => call(0, String(ms)),
    js_get_ws_url: (buf, max) => writeBack(call(9), buf, max),
  },
};

self.onmessage = async (e) => {
  // Dedicated worker: messages only originate from the same-origin context that created it. Reject any
  // cross-origin message defensively (the browser already guarantees this, but make the check explicit).
  if (e.origin && e.origin !== self.location.origin) return;
  const { sab } = e.data;
  ctrl = new Int32Array(sab, 0, 4);
  data = new Uint8Array(sab, DATA_OFFSET);
  // Resolve the module wasm from this worker's own location (self.location), never from a postMessage value,
  // so the fetch URL cannot depend on message data. The wasm is a fixed-name sibling of this worker script.
  const wasmUrl = new URL("et_ws_zig_except1.wasm", self.location.href);
  const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports);
  wasmMemory = instance.exports.memory;
  const ret = instance.exports.run();
  self.postMessage({ done: true, ret });
};
