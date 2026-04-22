// et_ws_zig_data1_worker.js — Web Worker for zig-data1 WASM module
const DATA_OFFSET = 16;
let ctrl, data, wasmMemory;
const enc = new TextEncoder(), dec = new TextDecoder();
const readStr = (ptr, len) => dec.decode(new Uint8Array(wasmMemory.buffer, ptr, len));
const writeData = (str) => {
  const b = enc.encode(str);
  data.set(b);
  return b.length;
};

function call(type, payload = "", aux = "") {
  const pb = enc.encode(payload), ab = enc.encode(aux);
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

const writeBack = (r, buf, max) => {
  const b = enc.encode(r);
  const n = Math.min(b.length, max);
  new Uint8Array(wasmMemory.buffer, buf, n).set(b.subarray(0, n));
  return n;
};

const imports = {
  env: {
    js_log: (p, l) => call(9, readStr(p, l)),
    js_set_status: (p, l) => call(10, readStr(p, l)),
    js_ws_connect: (p, l) => call(1, readStr(p, l)),
    js_ws_send: (p, l) => call(2, readStr(p, l)),
    js_ws_disconnect: () => call(8),
    js_ws_get_state: (buf, max) => writeBack(call(3), buf, max),
    js_ws_get_agent_id: (buf, max) => writeBack(call(4), buf, max),
    js_ws_pop_response: (buf, max) => {
      const r = call(5);
      return r ? writeBack(r, buf, max) : 0;
    },
    js_put_file: (up, ul, bp, bl) => call(6, readStr(up, ul), readStr(bp, bl)),
    js_get_file: (up, ul, buf, max) => writeBack(call(7, readStr(up, ul)), buf, max),
    js_sleep_ms: (ms) => call(0, String(ms)),
    js_get_ws_url: (buf, max) => writeBack(call(11), buf, max),
    js_get_iso_timestamp: (buf, max) => writeBack(call(12), buf, max),
  },
};

self.onmessage = async (e) => {
  const { sab, wasmUrl } = e.data;
  ctrl = new Int32Array(sab, 0, 4);
  data = new Uint8Array(sab, DATA_OFFSET);
  const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports);
  wasmMemory = instance.exports.memory;
  const ret = instance.exports.run();
  self.postMessage({ done: true, ret });
};
