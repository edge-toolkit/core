// et_ws_dart_comm1.js — ES module shim for dart-comm1

export default async function init() {
  await new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = new URL("et_ws_dart_comm1_compiled.js", import.meta.url).href;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
  });
}

export async function run() {
  if (typeof globalThis.dartComm1Run !== "function") {
    throw new Error("dart-comm1: not initialized");
  }
  // Dart @JS() interop resolves against globalThis, so expose the wasm-agent
  // classes there for the duration of the call.
  const { WsClient, WsClientConfig } = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  globalThis.WsClient = WsClient;
  globalThis.WsClientConfig = WsClientConfig;
  try {
    const result = globalThis.dartComm1Run();
    console.log("dart-comm1 dartComm1Run returned:", result, typeof result);
    await result;
  } catch (e) {
    console.error("dart-comm1 raw error:", e, "boxed:", e?.error);
    const msg = e?.error?.toString?.() ?? e?.message ?? String(e);
    throw new Error(msg);
  } finally {
    delete globalThis.WsClient;
    delete globalThis.WsClientConfig;
  }
}
