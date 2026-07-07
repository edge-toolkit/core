// XMLHttpRequest polyfill backed by fetch. Deno ships `fetch` but not
// `XMLHttpRequest`, and dio's web adapter (driving the generated `et_rest`
// Dart client used by dart-data1) routes every request through it. This
// implements only the surface `dio_web_adapter` touches: open/send/abort,
// setRequestHeader, responseType (always arraybuffer), timeout,
// withCredentials, the load/error/timeout events, and getAllResponseHeaders().
//
// Loaded after main.js by `shim_js()` in runtime.rs.
if (typeof globalThis.XMLHttpRequest === "undefined") {
  const UNSENT = 0;
  const OPENED = 1;
  const HEADERS_RECEIVED = 2;
  const LOADING = 3;
  const DONE = 4;

  globalThis.XMLHttpRequest = class XMLHttpRequest extends EventTarget {
    static UNSENT = UNSENT;
    static OPENED = OPENED;
    static HEADERS_RECEIVED = HEADERS_RECEIVED;
    static LOADING = LOADING;
    static DONE = DONE;

    readyState = UNSENT;
    status = 0;
    statusText = "";
    response = null;
    responseURL = "";
    responseType = "";
    withCredentials = false;
    timeout = 0;
    upload = new EventTarget();
    #method = "GET";
    #url = "";
    #headers = {};
    #responseHeaders = "";
    #controller = new AbortController();

    open(method, url) {
      this.#method = method;
      this.#url = url;
      this.readyState = OPENED;
    }

    setRequestHeader(key, value) {
      this.#headers[key] = value;
    }

    getAllResponseHeaders() {
      return this.#responseHeaders;
    }

    abort() {
      try {
        this.#controller.abort();
      } catch {
        /* already aborted */
      }
    }

    // skipcq: JS-R1005 -- XHR shim; complexity 6 is within the repo's oxlint ceiling (eslint/complexity max 10)
    send(body) {
      const base = globalThis.location?.href;
      const url = base ? new URL(this.#url, base).href : this.#url;
      const init = {
        method: this.#method,
        headers: this.#headers,
        signal: this.#controller.signal,
        credentials: this.withCredentials ? "include" : "same-origin",
      };
      if (body !== undefined && body !== null) {
        init.body = body;
      }
      let timer = null;
      if (this.timeout > 0) {
        timer = setTimeout(() => {
          this.abort();
          this.dispatchEvent(new Event("timeout"));
        }, this.timeout);
      }
      fetch(url, init)
        .then(async (resp) => {
          this.status = resp.status;
          this.statusText = resp.statusText;
          this.responseURL = resp.url;
          const lines = [];
          resp.headers.forEach((v, k) => lines.push(`${k}: ${v}`));
          this.#responseHeaders = lines.join("\r\n");
          this.readyState = HEADERS_RECEIVED;
          this.response = await resp.arrayBuffer();
          this.readyState = DONE;
          if (timer !== null) clearTimeout(timer);
          this.dispatchEvent(new Event("load"));
        })
        .catch((err) => {
          if (timer !== null) clearTimeout(timer);
          this.readyState = DONE;
          // AbortController.abort() rejects with an AbortError; dio's cancel and
          // timeout paths drive that and have already completed their futures.
          if (err?.name === "AbortError") return;
          this.dispatchEvent(new Event("error"));
        });
    }
  };
}
