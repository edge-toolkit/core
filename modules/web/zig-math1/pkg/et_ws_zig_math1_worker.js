// et_ws_zig_math1_worker.js -- Web Worker for zig-math1 WASM module
const DATA_OFFSET = 16;
// Bound to the SharedArrayBuffer and the instance's memory once `onmessage` receives them.
let ctrl = null;
let data = null;
let wasmMemory = null;
const enc = new TextEncoder(),
  dec = new TextDecoder();
const readStr = (ptr, len) => dec.decode(new Uint8Array(wasmMemory.buffer, ptr, len));

// String payload, response is a UTF-8 string.
const call = (type, payload = "") => {
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
};

// "METHOD url" payload plus a binary body; response is raw bytes, or null when
// the main thread signalled failure (negative response length).
const callRest = (method, url, body) => {
  const pb = enc.encode(`${method} ${url}`);
  const ab = body || new Uint8Array(0);
  data.set(pb);
  if (ab.length) data.set(ab, pb.length);
  Atomics.store(ctrl, 3, ab.length);
  Atomics.store(ctrl, 2, pb.length);
  Atomics.store(ctrl, 1, 11);
  Atomics.store(ctrl, 0, 1);
  Atomics.notify(ctrl, 0);
  Atomics.wait(ctrl, 0, 1);
  const rlen = Atomics.load(ctrl, 2);
  if (rlen < 0) return null;
  return Uint8Array.from(data.subarray(0, rlen));
};

const writeBack = (text, buf, max) => {
  const bytes = enc.encode(text);
  const count = Math.min(bytes.length, max);
  new Uint8Array(wasmMemory.buffer, buf, count).set(bytes.subarray(0, count));
  return count;
};

const imports = {
  env: {
    js_log: (ptr, len) => call(7, readStr(ptr, len)),
    js_set_status: (ptr, len) => call(8, readStr(ptr, len)),
    js_ws_connect: (ptr, len) => call(1, readStr(ptr, len)),
    js_ws_disconnect: () => call(6),
    js_ws_get_state: (buf, max) => writeBack(call(2), buf, max),
    js_ws_get_agent_id: (buf, max) => writeBack(call(3), buf, max),
    js_ws_get_input: (buf, max) => writeBack(call(4), buf, max),
    js_rest_request: (mp, ml, up, ul, bp, bl, buf, max) => {
      const method = readStr(mp, ml);
      const url = readStr(up, ul);
      const body = bl > 0 ? new Uint8Array(wasmMemory.buffer, bp, bl).slice() : null;
      const response = callRest(method, url, body);
      if (response === null) return -1;
      const count = Math.min(response.length, max);
      new Uint8Array(wasmMemory.buffer, buf, count).set(response.subarray(0, count));
      return count;
    },
    js_sleep_ms: (ms) => call(0, String(ms)),
    js_get_ws_url: (buf, max) => writeBack(call(9), buf, max),
  },
};

self.onmessage = async (event) => {
  // Dedicated worker: messages only originate from the same-origin context that created it. Reject any
  // cross-origin message defensively (the browser already guarantees this, but make the check explicit).
  if (event.origin && event.origin !== self.location.origin) return;
  const { sab } = event.data;
  ctrl = new Int32Array(sab, 0, 4);
  data = new Uint8Array(sab, DATA_OFFSET);
  // Resolve the module wasm from this worker's own location (self.location), never from a postMessage value,
  // so the fetch URL cannot depend on message data. The wasm is a fixed-name sibling of this worker script.
  const wasmUrl = new URL("et_ws_zig_math1.wasm", self.location.href);
  // Every failure still has to post `done`: instantiateStreaming can reject and run() can trap, and an
  // unhandled rejection here posts nothing at all, leaving the page waiting on a message that never arrives.
  try {
    const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports);
    wasmMemory = instance.exports.memory;
    const ret = instance.exports.run();
    self.postMessage({ done: true, ret });
  } catch (error) {
    self.postMessage({ done: true, error: String(error), ret: -1 });
  }
};
