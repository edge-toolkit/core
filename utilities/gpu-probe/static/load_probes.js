// The probes' WASM, started: the usual build, or where the browser's WebAssembly refuses it, as one with no reference
// types does, the build made for such browsers; where there is none, the usual build's refusal stands. A browser with
// no WebAssembly at all is given the legacy build instead, which has a module of its own in place of this one.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export const loadProbes = async () => {
  const name = "et_int_gpu_probe_web.js";
  const usual = await import(`./pkg/${name}`);
  try {
    await usual.default();
    return usual;
  } catch (error) {
    if (!(error instanceof WebAssembly.CompileError)) throw error;
    const compat = await import(`./pkg-compat/${name}`).catch(() => null);
    if (!compat) throw error;
    await compat.default();
    return compat;
  }
};
