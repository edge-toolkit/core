// et_ws_dart_math1.js -- ES module shim for dart-math1

// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
export default async function init() {
  await new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = new URL("et_ws_dart_math1_compiled.js", import.meta.url).href;
    script.onload = resolve;
    script.onerror = reject;
    document.head.appendChild(script);
  });
}

export async function run() {
  if (typeof globalThis.dartMath1Run !== "function") {
    throw new Error("dart-math1: not initialized");
  }
  // Dart @JS() interop resolves against globalThis, so expose the wasm-agent
  // classes there for the duration of the call. The FedAvg kernel itself is
  // pure local computation and needs no further globals.
  const wasmAgent = await import("/modules/@edge-toolkit/et-ws-wasm-agent/et_ws_wasm_agent.js");
  await wasmAgent.default();
  const { WsClient, WsClientConfig } = wasmAgent;
  globalThis.WsClient = WsClient;
  globalThis.WsClientConfig = WsClientConfig;
  try {
    const result = globalThis.dartMath1Run();
    console.log("dart-math1 dartMath1Run returned:", result, typeof result);
    await result;
  } catch (error) {
    console.error("dart-math1 raw error:", error, "boxed:", error?.error);
    const msg = error?.error?.toString?.() ?? error?.message ?? String(error);
    throw new Error(msg, { cause: error });
  } finally {
    delete globalThis.WsClient;
    delete globalThis.WsClientConfig;
  }
}
