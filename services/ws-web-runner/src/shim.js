// et-ws-web-runner browser-environment shim, layered on top of Deno's
// bootstrap. Placeholders __ET_HTTP_BASE__ and __ET_WS_URL__ are
// substituted by `shim_js()` in `runtime.rs` (plain literal replacement,
// nothing fancy) before the script is executed.

globalThis.__ET_WS_URL = "__ET_WS_URL__";
globalThis.__ET_HTTP_BASE = "__ET_HTTP_BASE__";

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
  } catch (_) { /* navigator is read-only in some setups, ignore */ }
}

// `document` stub -- enough for wasm-bindgen modules that probe DOM
// without actually touching it. dart2js's bootstrap takes the fast
// path if `document.currentScript` is defined (even null), bypassing
// the `document.scripts` load-event dance that needs real script tags.
if (typeof globalThis.document === "undefined") {
  const noop = () => {};
  class StubElement {
    constructor(tag) {
      this.tagName = (tag || "DIV").toUpperCase();
      this.children = [];
      this.style = {};
      this.classList = {
        add: noop,
        remove: noop,
        toggle: noop,
        contains: () => false,
      };
      this.textContent = "";
      this.innerHTML = "";
      this.value = "";
      this.hidden = false;
      this.src = "";
      this.id = "";
      this.className = "";
      this.type = "";
      this.onload = null;
      this.onerror = null;
    }
    appendChild(child) {
      this.children.push(child);
      // A `<script src=>` append fires a dynamic import() so loaded
      // module code actually runs (the Pyodide loader uses this).
      if (child.tagName === "SCRIPT" && child.src) {
        import(child.src).then(() => {
          if (typeof child.onload === "function") child.onload();
        }).catch((err) => {
          if (typeof child.onerror === "function") child.onerror(err);
        });
      }
      return child;
    }
    removeChild() {
      return this;
    }
    setAttribute(k, v) {
      this[k] = v;
    }
    getAttribute(k) {
      return this[k] ?? null;
    }
    addEventListener() {}
    removeEventListener() {}
    querySelector() {
      return null;
    }
    querySelectorAll() {
      return [];
    }
    dispatchEvent() {
      return true;
    }
    insertAdjacentHTML() {}
    getBoundingClientRect() {
      return { top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0 };
    }
    getContext() {
      return null;
    }
  }
  globalThis.document = {
    createElement: (tag) => new StubElement(tag),
    getElementById: () => new StubElement("div"),
    head: new StubElement("head"),
    body: new StubElement("body"),
    createTextNode: (text) => ({ textContent: text }),
    querySelectorAll: () => [],
    querySelector: () => null,
    addEventListener: noop,
    createEvent: () => ({ initEvent: noop }),
    createDocumentFragment: () => new StubElement("fragment"),
    currentScript: null,
    scripts: [],
  };
}

// HTMLElement / HTMLCanvasElement stubs. The HTMLCanvasElement
// Symbol.hasInstance lets wasm-bindgen's `HtmlCanvasElement::dyn_into`
// succeed on the StubElement above (its tagName === "CANVAS").
if (typeof globalThis.HTMLElement === "undefined") {
  globalThis.HTMLElement = class HTMLElement {};
}
if (typeof globalThis.HTMLCanvasElement === "undefined") {
  globalThis.HTMLCanvasElement = class HTMLCanvasElement {
    static [Symbol.hasInstance](instance) {
      return !!(instance && instance.tagName === "CANVAS");
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

// Pyodide calls globalThis.addEventListener("message", ...). Provide a
// minimal in-process event target if Deno hasn't installed one already.
if (typeof globalThis.addEventListener !== "function") {
  const listeners = {};
  globalThis.addEventListener = (type, fn) => {
    if (!listeners[type]) listeners[type] = [];
    listeners[type].push(fn);
  };
  globalThis.removeEventListener = (type, fn) => {
    if (!listeners[type]) return;
    listeners[type] = listeners[type].filter(f => f !== fn);
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

// WebAssembly streaming: deno_fetch Responses aren't recognised by V8's
// native streaming compile/instantiate, so fall back to arrayBuffer +
// instantiate/compile. Without this, the dotnet loader's
// `compileStreaming(fetch(...))` path throws.
{
  WebAssembly.instantiateStreaming = async (source, imports) => {
    const resp = await source;
    const bytes = await resp.arrayBuffer();
    return WebAssembly.instantiate(bytes, imports);
  };
  WebAssembly.compileStreaming = async (source) => {
    const resp = await source;
    const bytes = await resp.arrayBuffer();
    return WebAssembly.compile(bytes);
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
} catch (_) {}
