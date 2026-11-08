// et_ws_dart_data1.js -- ES module shim for dart-data1

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export default async function init() {
  await new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = new URL("et_ws_dart_data1_compiled.js", import.meta.url).href;
    script.onload = resolve;
    script.onerror = reject;
    document.head.appendChild(script);
  });
}

export async function run() {
  if (typeof globalThis.dartData1Run !== "function") {
    throw new Error("dart-data1: not initialized");
  }
  // Dart @JS() interop resolves against globalThis, so expose the wasm-agent
  // classes there for the duration of the call. The REST round-trip uses dio's
  // browser adapter directly and needs no globals.
  const wasmAgent = await import("/modules/@edge-toolkit/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  globalThis.WsClient = WsClient;
  globalThis.WsClientConfig = WsClientConfig;
  try {
    const result = globalThis.dartData1Run();
    console.log("dart-data1 dartData1Run returned:", result, typeof result);
    await result;
  } catch (error) {
    console.error("dart-data1 raw error:", error, "boxed:", error?.error);
    const msg = error?.error?.toString?.() ?? error?.message ?? String(error);
    throw new Error(msg, { cause: error });
  } finally {
    delete globalThis.WsClient;
    delete globalThis.WsClientConfig;
  }
}
