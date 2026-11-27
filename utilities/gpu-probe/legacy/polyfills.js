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

// An error thrown again outside the promise it was caught in, so the browser reports it as uncaught.
const rethrow = (error) => {
  setTimeout(() => {
    throw error;
  });
};

// A task that throws is reported as an uncaught error, as the browser's own reports it.
define(window, "queueMicrotask", (task) => {
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

// Each parameter of a query string, as a name and a value.
const queryPairs = (search) => String(search).replace(/^\?/, "").split("&").filter(Boolean).map(nameAndValue);

// Part of a query encoded as a form encodes it, a space as `+`.
const formEncoded = (part) => encodeURIComponent(part).replace(/%20/g, "+");

// The query parameters the page reads, by name, the first of each name kept: from the page's query string, or from an
// object of them, as a proposal's link is made.
define(window, "URLSearchParams", function URLSearchParams(init) {
  this.values = {};
  for (const [name, value] of typeof init === "object" ? Object.entries(init) : queryPairs(init)) {
    if (!(name in this.values)) this.values[name] = String(value);
  }
});
define(URLSearchParams.prototype, "get", function get(name) {
  return name in this.values ? this.values[name] : null;
});
define(URLSearchParams.prototype, "has", function has(name) {
  return name in this.values;
});
// Every prototype inherits a toString, so the polyfill's own is put where its prototype has none of its own.
if (!Object.prototype.hasOwnProperty.call(URLSearchParams.prototype, "toString")) {
  put(URLSearchParams.prototype, "toString", function toString() {
    const pairs = Object.keys(this.values).map((name) => `${formEncoded(name)}=${formEncoded(this.values[name])}`);
    return pairs.join("&");
  });
}

// SHA-256 alone, which is all the page asks a digest of: it names each proposed file after the start of its hash.
// Its constants are the first 32 bits of the fractional parts of the square roots of the first 8 primes, which start
// the hash, and of the cube roots of the first 64, one for each round.
const primes = (count) => {
  const found = [];
  for (let candidate = 2; found.length < count; candidate += 1) {
    if (found.every((prime) => candidate % prime)) found.push(candidate);
  }
  return found;
};
const fractionBits = (root) => ((root - Math.floor(root)) * 2 ** 32) >>> 0;
const initialHash = primes(8).map((prime) => fractionBits(Math.sqrt(prime)));
const roundConstants = primes(64).map((prime) => fractionBits(Math.cbrt(prime)));

const rotate = (word, by) => (word >>> by) | (word << (32 - by));

// The 64 words a block's rounds take, its own 16 then 48 mixed from those before.
const schedule = (block) => {
  const words = new Uint32Array(64);
  words.set(block);
  for (let index = 16; index < 64; index += 1) {
    const early = words[index - 15];
    const late = words[index - 2];
    const mixedEarly = rotate(early, 7) ^ rotate(early, 18) ^ (early >>> 3);
    const mixedLate = rotate(late, 17) ^ rotate(late, 19) ^ (late >>> 10);
    words[index] = words[index - 16] + mixedEarly + words[index - 7] + mixedLate;
  }
  return words;
};

// One block's 64 rounds over the working state, the standard's `a` to `h`, added into `hash`.
const compress = (hash, block) => {
  const words = schedule(block);
  const state = Array.from(hash);
  for (let round = 0; round < 64; round += 1) {
    const [wordA, wordB, wordC, , wordE, wordF, wordG, wordH] = state;
    const chosen = (wordE & wordF) ^ (~wordE & wordG);
    const mixedE = rotate(wordE, 6) ^ rotate(wordE, 11) ^ rotate(wordE, 25);
    const first = wordH + mixedE + chosen + roundConstants[round] + words[round];
    const majority = (wordA & wordB) ^ (wordA & wordC) ^ (wordB & wordC);
    const second = (rotate(wordA, 2) ^ rotate(wordA, 13) ^ rotate(wordA, 22)) + majority;
    state.pop();
    state.unshift((first + second) >>> 0);
    state[4] = (state[4] + first) >>> 0;
  }
  for (let index = 0; index < 8; index += 1) hash[index] += state[index];
};

// The SHA-256 of `bytes`: padded with a 1 bit, zeros and its length in bits to whole 64-byte blocks, read big-endian.
const sha256 = (bytes) => {
  const blocks = Math.ceil((bytes.length + 9) / 64);
  const words = new Uint32Array(blocks * 16);
  for (let index = 0; index < bytes.length; index += 1) words[index >> 2] |= bytes[index] << (24 - (index % 4) * 8);
  words[bytes.length >> 2] |= 0x80 << (24 - (bytes.length % 4) * 8);
  words[words.length - 2] = Math.floor(bytes.length / 2 ** 29);
  words[words.length - 1] = bytes.length * 8;
  const hash = new Uint32Array(initialHash);
  for (let block = 0; block < blocks; block += 1) compress(hash, words.subarray(block * 16, block * 16 + 16));
  const digest = new Uint8Array(32);
  for (let index = 0; index < 32; index += 1) digest[index] = hash[index >> 2] >>> (24 - (index % 4) * 8);
  return digest;
};

// Tested by value, not by name: a browser can name `subtle` and give none, as Chrome does on a page served insecurely.
define(window, "crypto", {});
if (!crypto.subtle) {
  put(crypto, "subtle", { digest: (_algorithm, data) => Promise.resolve(sha256(new Uint8Array(data)).buffer) });
}

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
