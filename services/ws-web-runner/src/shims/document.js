// `document` stub -- enough for wasm-bindgen modules that probe DOM
// without actually touching it. dart2js's bootstrap takes the fast
// path if `document.currentScript` is defined (even null), bypassing
// the `document.scripts` load-event dance that needs real script tags.
//
// Loaded after main.js by `shim_js()` in runtime.rs.
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
        import(child.src)
          .then(() => {
            if (typeof child.onload === "function") child.onload();
          })
          .catch((err) => {
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
