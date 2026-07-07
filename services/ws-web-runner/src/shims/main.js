// et-ws-web-runner browser-environment shim, layered on top of Deno's
// bootstrap. `shim_js()` in `runtime.rs` emits the `globalThis.__ET_HTTP_BASE`
// / `globalThis.__ET_WS_URL` globals before this file, then concatenates the
// other shims/*.js fragments after it (document, events, wasm-streaming, xhr).
//
// This file holds the small global-identity glue; the larger concerns live in
// their own fragments.

// Browser-only Symbol.hasInstance trick: wasm-bindgen's
// `globalThis instanceof Window` check.
if (typeof globalThis.Window === "undefined") {
  globalThis.Window = class Window {
    static [Symbol.hasInstance](instance) {
      return instance === globalThis || instance === globalThis.window;
    }
  };
}

// `self` / `window` aliases (Deno provides `self`; alias `window`).
globalThis.window = globalThis;

// NOTE: `location` is already populated via `BootstrapOptions.location =
// http_base`, and deno_web's location is read-only -- assignments throw
// `NotSupportedError: Cannot set "location"`. Don't override here.

// `navigator.userAgent`. Deno already provides `navigator`, just add a
// useful UA string if it's missing.
if (typeof globalThis.navigator === "object" && !globalThis.navigator.userAgent) {
  try {
    Object.defineProperty(globalThis.navigator, "userAgent", {
      value: "et-ws-web-runner/deno",
      configurable: true,
    });
  } catch {
    /* navigator is read-only in some setups, ignore */
  }
}

// HTMLElement / HTMLCanvasElement stubs. The HTMLCanvasElement
// Symbol.hasInstance lets wasm-bindgen's `HtmlCanvasElement::dyn_into`
// succeed on the StubElement in document.js (its tagName === "CANVAS").
if (typeof globalThis.HTMLElement === "undefined") {
  globalThis.HTMLElement = class HTMLElement {};
}
if (typeof globalThis.HTMLCanvasElement === "undefined") {
  globalThis.HTMLCanvasElement = class HTMLCanvasElement {
    static [Symbol.hasInstance](instance) {
      return instance?.tagName === "CANVAS";
    }
  };
}
if (typeof globalThis.Image === "undefined") {
  globalThis.Image = class Image {
    constructor() {
      this.src = "";
    }
  };
}

// Hide Deno from module code. Pyodide and other libs sniff `typeof Deno`
// and take a path that doesn't work in an embedded runtime.
delete globalThis.Deno;

// Hide `process` too -- deno_runtime exposes a Node-style `process`
// global via deno_node, and Pyodide treats `typeof process === "object"`
// + `process.versions.node` as proof it's running under Node and tries
// to load its WASM via a node FS path (file://). The browser branch
// uses fetch() against the http_base, which is what we want.
try {
  delete globalThis.process;
} catch {
  /* delete may be forbidden in strict mode -- harmless */
}
