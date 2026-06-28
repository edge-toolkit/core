// et_ws_dart_data1.js -- ES module shim for dart-data1

export default async function init() {
  await new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = new URL("et_ws_dart_data1_compiled.js", import.meta.url).href;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
  });
}

export async function run() {
  if (typeof globalThis.dartData1Run !== "function") {
    throw new Error("dart-data1: not initialized");
  }
  // Dart @JS() interop resolves against globalThis, so expose the wasm-agent
  // classes there for the duration of the call. The REST round-trip uses dio's
  // browser adapter directly and needs no globals.
  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  globalThis.WsClient = WsClient;
  globalThis.WsClientConfig = WsClientConfig;
  try {
    const result = globalThis.dartData1Run();
    console.log("dart-data1 dartData1Run returned:", result, typeof result);
    await result;
  } catch (e) {
    console.error("dart-data1 raw error:", e, "boxed:", e?.error);
    const msg = e?.error?.toString?.() ?? e?.message ?? String(e);
    throw new Error(msg, { cause: e });
  } finally {
    delete globalThis.WsClient;
    delete globalThis.WsClientConfig;
  }
}
