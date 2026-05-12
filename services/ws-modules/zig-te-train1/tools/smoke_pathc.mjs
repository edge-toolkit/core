// Headless Path C smoke test. Drives the wasm's smoke_* exports
// (defined in src/main.zig) via Node's WASI reactor binding. Verifies:
//   1. Backbone has not feature-collapsed (pool delta on black vs white)
//   2. default binary prototype training produces a non-zero margin
//   3. te_train_step mutates the expected v* regions
//   4. te_reset_weights restores the snapshot cleanly
//
// Usage (must be a ReleaseFast build — Debug hits UBSan on misaligned
// q31_t accesses inside the int_forward_op kernels):
//
//   zig build -Doptimize=ReleaseFast
//   node tools/smoke_pathc.mjs pkg/et_ws_zig_te_train1.wasm

import { readFile } from "node:fs/promises";
import { WASI } from "node:wasi";

const wasm = await readFile(process.argv[2]);
const wasi = new WASI({ version: "preview1", args: [], env: {}, returnOnExit: true });
const importObject = {
  env: {
    js_log: (ptr, len) => {
      try {
        const m = new Uint8Array(inst.exports.memory.buffer, ptr, len);
        process.stderr.write("[js_log] " + new TextDecoder().decode(m) + "\n");
      } catch (e) {}
    },
    js_set_status: () => {},
    js_sleep_ms: () => {},
    js_ws_connect: () => {},
    js_ws_send: () => {},
    js_ws_disconnect: () => {},
    js_ws_get_state: () => 0,
    js_ws_get_agent_id: () => 0,
    js_ws_pop_response: () => 0,
    js_get_ws_url: () => 0,
    js_get_file_bin: () => 0,
    js_put_file: () => {},
    te_arena_fill_canary: () => {},
    te_arena_count_touched: () => 0,
  },
  wasi_snapshot_preview1: wasi.wasiImport,
};

const mod = await WebAssembly.compile(wasm);
const inst = await WebAssembly.instantiate(mod, importObject);
// Reactor-style WASI initialize — allows calling exports without _start's main loop.
try {
  wasi.initialize(inst);
} catch (e) {
  console.log("initialize fallback:", e.message);
}
const e = inst.exports;
const mem = e.memory;

const r = e.smoke_init();
console.log("smoke_init:", r);
const inputPtr = e.smoke_input_ptr();
const inputSize = e.smoke_input_size();
const W = e.smoke_input_width(), H = e.smoke_input_height(), C = e.smoke_input_channels();
console.log(`input: ${W}x${H}x${C} = ${inputSize} bytes at ptr=${inputPtr}`);

const HEAP_BASE = 24 * 1024 * 1024;
const sigPtr = HEAP_BASE,
  debugPtr = HEAP_BASE + 256,
  scoresPtr = HEAP_BASE + 512,
  logitsPtr = HEAP_BASE + 768,
  binaryPtr = HEAP_BASE + 1024;
function readI8(p, n) {
  return Array.from(new Int8Array(mem.buffer, p, n));
}
function readI32(p, n) {
  return Array.from(new Int32Array(mem.buffer, p, n));
}
function fillI8(v) {
  new Int8Array(mem.buffer, inputPtr, inputSize).fill(v);
}

console.log("\n=== EXTREMES VERIFIER ===");
fillI8(-128);
e.smoke_run_inference();
e.smoke_get_pooled_sig(sigPtr, 16);
const poolBlack = readI8(sigPtr, 16);
e.smoke_get_logits(logitsPtr);
const logitsBlack = readI8(logitsPtr, 10);
e.smoke_get_scores(scoresPtr);
const scoresBlack = readI32(scoresPtr, 10);

fillI8(127);
e.smoke_run_inference();
e.smoke_get_pooled_sig(sigPtr, 16);
const poolWhite = readI8(sigPtr, 16);
e.smoke_get_logits(logitsPtr);
const logitsWhite = readI8(logitsPtr, 10);
e.smoke_get_scores(scoresPtr);
const scoresWhite = readI32(scoresPtr, 10);

console.log("pool  black[0..16]:", poolBlack);
console.log("pool  white[0..16]:", poolWhite);
let nDiff = 0, maxDiff = 0;
for (let i = 0; i < 16; i++) {
  const d = poolBlack[i] - poolWhite[i];
  if (d !== 0) nDiff++;
  if (Math.abs(d) > maxDiff) maxDiff = Math.abs(d);
}
console.log(`pool diff: ${nDiff}/16 positions, max |Δ|=${maxDiff}`);
console.log("logits black:", logitsBlack);
console.log("logits white:", logitsWhite);

let scoreMaxDiff = 0;
for (let i = 0; i < 10; i++) {
  const d = Math.abs(scoresBlack[i] - scoresWhite[i]);
  if (d > scoreMaxDiff) scoreMaxDiff = d;
}
console.log(`score max |Δ|: ${scoreMaxDiff}`);
if (maxDiff <= 1) console.log("⚠ FEATURE COLLAPSE: pool near-identical");
else console.log(`✓ Pool differs (max=${maxDiff})`);

console.log("\n=== BINARY PROTOTYPE SMOKE TEST ===");
e.smoke_reset();
fillI8(127);
for (let i = 0; i < 4; i++) e.smoke_train_binary_step(1);
fillI8(-128);
for (let i = 0; i < 4; i++) e.smoke_train_binary_step(0);

fillI8(127);
e.smoke_run_inference();
e.smoke_get_binary_scores(binaryPtr);
const binWhite = readI32(binaryPtr, 2);
fillI8(-128);
e.smoke_run_inference();
e.smoke_get_binary_scores(binaryPtr);
const binBlack = readI32(binaryPtr, 2);
console.log("binary white:", binWhite, "pred=", binWhite[1] >= binWhite[0] ? 1 : 0);
console.log("binary black:", binBlack, "pred=", binBlack[1] >= binBlack[0] ? 1 : 0);
console.log("binary margins:", { white: binWhite[1] - binWhite[0], black: binBlack[1] - binBlack[0] });

console.log("\n=== SPARSE-UPDATE SMOKE TEST ===");
e.smoke_reset();
e.smoke_get_train_debug(debugPtr);
const pre = readI32(debugPtr, 16);
console.log("pre:", { snap: pre[15], total: pre[0], hw: pre[3], hb: pre[6], bw: pre[9], bb: pre[12] });

fillI8(127);
for (let i = 0; i < 3; i++) e.smoke_train_step(3);

e.smoke_get_train_debug(debugPtr);
const post = readI32(debugPtr, 16);
console.log("post:", { snap: post[15], total: post[0], hw: post[3], hb: post[6], bw: post[9], bb: post[12] });
console.log(`mutated ${post[0]} bytes / hw=${post[3]} / hb=${post[6]} / bw=${post[9]} / bb=${post[12]}`);

e.smoke_reset();
e.smoke_get_train_debug(debugPtr);
const reset = readI32(debugPtr, 16);
console.log("after reset:", { total: reset[0] }, reset[0] === 0 ? "✓ snapshot restored" : "⚠ leak");
