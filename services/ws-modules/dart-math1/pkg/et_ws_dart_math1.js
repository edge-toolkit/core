// et_ws_dart_math1.js -- ES module shim for dart-math1

export default async function init() {
  await new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = new URL("et_ws_dart_math1_compiled.js", import.meta.url).href;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
  });
}

export async function run() {
  if (typeof globalThis.dartMath1Run !== "function") {
    throw new Error("dart-math1: not initialized");
  }
  // Dart @JS() interop resolves against globalThis, so expose the wasm-agent
  // classes there for the duration of the call. The FedAvg kernel itself is
  // pure local computation and needs no further globals.
  const wasmAgent = await import("/modules/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  globalThis.WsClient = WsClient;
  globalThis.WsClientConfig = WsClientConfig;
  try {
    const result = globalThis.dartMath1Run();
    console.log("dart-math1 dartMath1Run returned:", result, typeof result);
    await result;
  } catch (e) {
    console.error("dart-math1 raw error:", e, "boxed:", e?.error);
    const msg = e?.error?.toString?.() ?? e?.message ?? String(e);
    throw new Error(msg, { cause: e });
  } finally {
    delete globalThis.WsClient;
    delete globalThis.WsClientConfig;
  }
}
