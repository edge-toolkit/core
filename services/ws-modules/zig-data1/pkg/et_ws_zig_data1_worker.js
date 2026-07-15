// et_ws_zig_data1_worker.js — Web Worker for zig-data1 WASM module
const DATA_OFFSET = 16;
let ctrl, data, wasmMemory;
const enc = new TextEncoder(),
  dec = new TextDecoder();
const readStr = (ptr, len) => dec.decode(new Uint8Array(wasmMemory.buffer, ptr, len));

// String payload + string aux, response is a UTF-8 string (legacy ops).
function call(type, payload = "", aux = "") {
  const pb = enc.encode(payload),
    ab = enc.encode(aux);
  data.set(pb);
  if (ab.length) data.set(ab, pb.length);
  Atomics.store(ctrl, 3, ab.length);
  Atomics.store(ctrl, 2, pb.length);
  Atomics.store(ctrl, 1, type);
  Atomics.store(ctrl, 0, 1);
  Atomics.notify(ctrl, 0);
  Atomics.wait(ctrl, 0, 1); // block until main thread responds
  const rlen = Atomics.load(ctrl, 2);
  return dec.decode(Uint8Array.from(data.subarray(0, rlen)));
}

// REST request: method + url in the string payload (space-separated), body
// is *binary* aux bytes, response is *binary* bytes copied out of the SAB.
// Type 11 lives alongside the string-only legacy ops (0-10) so the existing
// dispatch table doesn't have to be reshuffled.
function callRest(method, url, body) {
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
  // Negative response length is the error sentinel — the main-thread
  // dispatch encodes (max int32 + 1 - n) to signal failure.
  if (rlen < 0) return null;
  // Slice copies the bytes out of the SAB region so the wasm caller can
  // overwrite the area on its next request.
  return new Uint8Array(data.subarray(0, rlen)).slice();
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
    // Single HTTP entry point used by the generated et_rest_client. The Zig
    // signature is: (method_ptr, method_len, url_ptr, url_len, body_ptr,
    // body_len, response_buf, response_max) -> i32. Returns bytes written
    // to response_buf, or -1 on failure.
    js_rest_request: (mp, ml, up, ul, bp, bl, buf, max) => {
      const method = readStr(mp, ml);
      const url = readStr(up, ul);
      const body = bl > 0 ? new Uint8Array(wasmMemory.buffer, bp, bl).slice() : null;
      const response = callRest(method, url, body);
      if (response === null) return -1;
      const n = Math.min(response.length, max);
      new Uint8Array(wasmMemory.buffer, buf, n).set(response.subarray(0, n));
      return n;
    },
    js_sleep_ms: (ms) => call(0, String(ms)),
    js_get_ws_url: (buf, max) => writeBack(call(9), buf, max),
    js_get_iso_timestamp: (buf, max) => writeBack(call(10), buf, max),
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
  const wasmUrl = new URL("et_ws_zig_data1.wasm", self.location.href);
  const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports);
  wasmMemory = instance.exports.memory;
  const ret = instance.exports.run();
  self.postMessage({ done: true, ret });
};
