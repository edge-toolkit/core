// et_ws_zig_te_train1_worker.js — Web Worker host for the wasm module.
// Communicates with the main thread via SharedArrayBuffer.
//
// Adds js_get_file_bin (request type 13) which writes raw image bytes from
// fetch() directly into wasm memory without TextDecoder round-tripping.

const DATA_OFFSET = 16;
let ctrl, data, wasmMemory;
const enc = new TextEncoder();
const dec = new TextDecoder();

const readStr = (ptr, len) => dec.decode(new Uint8Array(wasmMemory.buffer, ptr, len));

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
  return rlen;
}

const callStr = (type, payload = "", aux = "") => {
  const rlen = call(type, payload, aux);
  return dec.decode(Uint8Array.from(data.subarray(0, rlen)));
};

const writeBack = (str, buf, max) => {
  const b = enc.encode(str);
  const n = Math.min(b.length, max);
  new Uint8Array(wasmMemory.buffer, buf, n).set(b.subarray(0, n));
  return n;
};

// WASI imports — Zig's wasm32-wasi target pulls these in via libc/musl. We
// don't actually run any WASI syscalls (no file I/O, no stdout), so each stub
// returns errno=8 (WASI_EBADF) or 0 as appropriate. random_get is the one
// exception — if libc rand() ever gets called we want it to do something.
const ERRNO_BADF = 8;
const wasi = {
  args_get: () => 0,
  args_sizes_get: (ac, av) => {
    new DataView(wasmMemory.buffer).setUint32(ac, 0, true);
    new DataView(wasmMemory.buffer).setUint32(av, 0, true);
    return 0;
  },
  clock_res_get: () => 0,
  clock_time_get: (id, prec, out) => {
    const ns = BigInt(Date.now()) * 1000000n;
    new DataView(wasmMemory.buffer).setBigUint64(out, ns, true);
    return 0;
  },
  fd_close: () => 0,
  fd_fdstat_get: () => ERRNO_BADF,
  fd_filestat_get: () => ERRNO_BADF,
  fd_filestat_set_size: () => ERRNO_BADF,
  fd_filestat_set_times: () => ERRNO_BADF,
  fd_pread: () => ERRNO_BADF,
  fd_prestat_get: () => ERRNO_BADF,
  fd_prestat_dir_name: () => ERRNO_BADF,
  fd_pwrite: () => ERRNO_BADF,
  fd_read: () => ERRNO_BADF,
  fd_readdir: () => ERRNO_BADF,
  fd_seek: () => ERRNO_BADF,
  fd_sync: () => 0,
  fd_write: () => ERRNO_BADF,
  path_create_directory: () => ERRNO_BADF,
  path_filestat_get: () => ERRNO_BADF,
  path_filestat_set_times: () => ERRNO_BADF,
  path_link: () => ERRNO_BADF,
  path_open: () => ERRNO_BADF,
  path_readlink: () => ERRNO_BADF,
  path_remove_directory: () => ERRNO_BADF,
  path_rename: () => ERRNO_BADF,
  path_symlink: () => ERRNO_BADF,
  path_unlink_file: () => ERRNO_BADF,
  poll_oneoff: () => ERRNO_BADF,
  proc_exit: (code) => {
    // wasm wants to terminate (libc abort() / panic / unwind). Throw so the
    // wasm stack unwinds; the outer catch in onmessage posts a structured
    // {done, error} back to the main thread so the UI can surface the cause
    // instead of reporting "[object Event]". Including code in the message
    // makes it greppable.
    throw new Error("wasm proc_exit(" + code + ")");
  },
  random_get: (buf, len) => {
    const view = new Uint8Array(wasmMemory.buffer, buf, len);
    if (globalThis.crypto?.getRandomValues) crypto.getRandomValues(view);
    else for (let i = 0; i < len; i++) view[i] = (Math.random() * 256) | 0;
    return 0;
  },
};

const imports = {
  wasi_snapshot_preview1: wasi,
  env: {
    js_log: (p, l) => callStr(9, readStr(p, l)),
    js_set_status: (p, l) => callStr(10, readStr(p, l)),
    js_ws_connect: (p, l) => callStr(1, readStr(p, l)),
    js_ws_send: (p, l) => callStr(2, readStr(p, l)),
    js_ws_disconnect: () => callStr(8),
    js_ws_get_state: (buf, max) => writeBack(callStr(3), buf, max),
    js_ws_get_agent_id: (buf, max) => writeBack(callStr(4), buf, max),
    js_ws_pop_response: (buf, max) => {
      const r = callStr(5);
      return r ? writeBack(r, buf, max) : 0;
    },
    js_put_file: (up, ul, bp, bl) => callStr(6, readStr(up, ul), readStr(bp, bl)),
    js_get_file_bin: (up, ul, buf, max) => {
      // Type 13 writes raw bytes to the SAB data area without decoding.
      const rlen = call(13, readStr(up, ul));
      const n = Math.min(rlen, max);
      new Uint8Array(wasmMemory.buffer, buf, n).set(data.subarray(0, n));
      return n;
    },
    js_sleep_ms: (ms) => callStr(0, String(ms)),
    js_get_ws_url: (buf, max) => writeBack(callStr(11), buf, max),
  },
};

self.onmessage = async (e) => {
  const { sab, wasmUrl } = e.data;
  ctrl = new Int32Array(sab, 0, 4);
  data = new Uint8Array(sab, DATA_OFFSET);
  try {
    const { instance } = await WebAssembly.instantiateStreaming(
      fetch(wasmUrl),
      imports,
    );
    wasmMemory = instance.exports.memory;
    const ret = instance.exports.run();
    self.postMessage({ done: true, ret });
  } catch (err) {
    // Send structured error info to the main thread BEFORE the worker dies,
    // so the UI can show something useful instead of "[object Event]".
    // Common cause: a wasm trap, a libc abort()→proc_exit, or a thrown
    // exception from inside an import (we deliberately throw on proc_exit
    // to terminate wasm execution).
    const info = {
      done: true,
      ret: -1,
      error: {
        message: String(err?.message ?? err),
        name: err?.name ?? "Error",
        stack: err?.stack ?? null,
      },
    };
    try {
      self.postMessage(info);
    } catch {}
    throw err; // let it propagate to surface in onerror as well
  }
};
