// Pyodide calls globalThis.addEventListener("message", ...). Provide a
// minimal in-process event target if Deno hasn't installed one already.
//
// Loaded after main.js by `shim_js()` in runtime.rs.
if (typeof globalThis.addEventListener !== "function") {
  const listeners = {};
  globalThis.addEventListener = (type, fn) => {
    if (!listeners[type]) listeners[type] = [];
    listeners[type].push(fn);
  };
  globalThis.removeEventListener = (type, fn) => {
    if (!listeners[type]) return;
    listeners[type] = listeners[type].filter((f) => f !== fn);
  };
  globalThis.dispatchEvent = (evt) => {
    const type = evt.type || evt;
    if (!listeners[type]) return true;
    for (const fn of listeners[type]) fn(evt);
    return true;
  };
  globalThis.postMessage = (data) => {
    const evt = { type: "message", data, ports: [] };
    globalThis.dispatchEvent(evt);
  };
}
