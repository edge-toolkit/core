// The probes for a browser without WebAssembly: the same Rust, turned into JavaScript by wasm2js, which is ready as
// soon as it is loaded. The legacy build puts this module where the page imports /load_probes.js.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
import * as probes from "/pkg-legacy/et_int_gpu_probe_web.js";

export const loadProbes = async () => probes;
