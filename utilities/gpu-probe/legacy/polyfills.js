// What the probe page and the probes' wasm-bindgen glue use that Safari 9, the oldest browser the legacy build is for,
// lacks: each is put in place only where the browser has none of its own, and only as far as the page and the glue
// use it.

const put = (owner, name, value) => {
  Object.defineProperty(owner, name, { configurable: true, writable: true, value });
};
const define = (owner, name, value) => {
  if (!(name in owner)) put(owner, name, value);
};

// Whether the browser's own fetch, where it has one, gives up on a request when told to.
// One from before AbortController ignores the signal the polyfill below makes, which would leave the page waiting on a
// status post the server never answers. Safari 10.3 has such a fetch, so there fetch is replaced as well.
const abortable = "AbortController" in window;

define(window, "globalThis", window);

// A task that throws is reported as an uncaught error, as the browser's own reports it.
define(window, "queueMicrotask", (task) => {
  const rethrow = (error) => {
    setTimeout(() => {
      throw error;
    });
  };
  Promise.resolve().then(task).catch(rethrow);
});

define(window, "Reflect", {});
define(Reflect, "set", (target, key, value) => {
  target[key] = value;
  return true;
});

define(Object, "entries", (object) => Object.keys(object).map((key) => [key, object[key]]));
define(Object, "values", (object) => Object.keys(object).map((key) => object[key]));

define(String.prototype, "padStart", function padStart(length, filler) {
  let padded = String(this);
  while (padded.length < length) padded = filler + padded;
  return padded.slice(padded.length - Math.max(length, String(this).length));
});

// UTF-8 alone, which is all the glue asks for.
define(window, "TextEncoder", function TextEncoder() {
  this.encoding = "utf-8";
});
define(TextEncoder.prototype, "encode", (text = "") => {
  const bytes = unescape(encodeURIComponent(text));
  const encoded = new Uint8Array(bytes.length);
  for (let index = 0; index < bytes.length; index += 1) encoded[index] = bytes.charCodeAt(index);
  return encoded;
});
define(window, "TextDecoder", function TextDecoder() {
  this.encoding = "utf-8";
});
define(TextDecoder.prototype, "decode", (bytes) => {
  if (!bytes) return "";
  let text = "";
  for (let index = 0; index < bytes.length; index += 1) text += String.fromCharCode(bytes[index]);
  return decodeURIComponent(escape(text));
});

const node = (item) => (typeof item === "string" ? document.createTextNode(item) : item);
define(Element.prototype, "append", function append(...items) {
  for (const item of items) this.appendChild(node(item));
});
define(Element.prototype, "after", function after(...items) {
  const next = this.nextSibling;
  for (const item of items) this.parentNode.insertBefore(node(item), next);
});
define(Element.prototype, "replaceChildren", function replaceChildren(...items) {
  while (this.firstChild) this.removeChild(this.firstChild);
  this.append(...items);
});

// Part of a query decoded as a form is, `+` as a space; one encoded wrongly is kept as sent, not failing the page.
const formDecoded = (part) => {
  const spaced = part.replace(/\+/g, " ");
  try {
    return decodeURIComponent(spaced);
  } catch {
    return spaced;
  }
};

// A query parameter's name and value, decoded, split at its first `=`; with none, its value is empty.
const nameAndValue = (pair) => {
  const split = pair.indexOf("=");
  if (split < 0) return [formDecoded(pair), ""];
  return [formDecoded(pair.slice(0, split)), formDecoded(pair.slice(split + 1))];
};

// The query parameters the page reads, by name, the first of each name kept.
define(window, "URLSearchParams", function URLSearchParams(search) {
  this.values = {};
  for (const pair of String(search).replace(/^\?/, "").split("&")) {
    if (!pair) continue;
    const [name, value] = nameAndValue(pair);
    if (!(name in this.values)) this.values[name] = value;
  }
});
define(URLSearchParams.prototype, "get", function get(name) {
  return name in this.values ? this.values[name] : null;
});
define(URLSearchParams.prototype, "has", function has(name) {
  return name in this.values;
});

// A signal aborted once, which a request it is given then gives up on.
define(window, "AbortController", function AbortController() {
  this.signal = { aborted: false, listeners: [] };
});
define(AbortController.prototype, "abort", function abort() {
  this.signal.aborted = true;
  for (const listener of this.signal.listeners) listener();
});

// A request through XMLHttpRequest, answered with as much of a Response as the page reads.
(abortable ? define : put)(
  window,
  "fetch",
  (url, options = {}) =>
    new Promise((resolve, reject) => {
      const request = new XMLHttpRequest();
      request.open(options.method || "GET", url);
      for (const name of Object.keys(options.headers || {})) {
        request.setRequestHeader(name, options.headers[name]);
      }
      request.addEventListener("load", () => {
        const text = request.responseText;
        resolve({
          ok: request.status >= 200 && request.status < 300,
          status: request.status,
          text: () => Promise.resolve(text),
          json: () => Promise.resolve().then(() => JSON.parse(text)),
        });
      });
      request.addEventListener("error", () => reject(new TypeError(`the request to ${url} failed`)));
      request.addEventListener("abort", () => reject(new Error(`the request to ${url} was aborted`)));
      if (options.signal) options.signal.listeners.push(() => request.abort());
      request.send(options.body);
    }),
);
