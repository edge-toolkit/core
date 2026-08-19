// js-data1 -- a JavaScript twin of the Rust `data1` module that drives the per-agent storage round-trip
// through the AWS SDK for JavaScript v3 (`@aws-sdk/client-s3`) instead of the generated REST client.
//
// The point of this module is to prove that `et-storage-service` is an anonymous S3-compatible interface. We
// point a real S3 client at it -- endpoint `${httpBase}/storage`, path-style, bucket = agent_id, key = filename
// -- so `PutObject`/`GetObject`/`HeadObject` map straight onto the `PUT`/`GET`/`HEAD /storage/{agent_id}/{filename}`
// routes. All three are verified: the object round-trips byte-for-byte, and PUT/HEAD both return an `ETag`.
//
// The module contract (see services/ws-web-runner/src/runtime.rs and services/ws-server/static/app.js): export
// a `default` async init and an async `run`. `run` resolving == success (runner exits 0); `run` throwing ==
// failure (runner exits non-zero). We throw only on the core round-trip failing, never on an S3 feature the
// service is simply missing.

import { GetObjectCommand, HeadObjectCommand, PutObjectCommand, S3Client } from "@aws-sdk/client-s3";

const MODULE = "js-data1";
const FILENAME = "test_data.txt";

// Log to the console (prefixed) and, in a real browser, mirror into the on-page status textarea.
function log(message) {
  console.log(`[${MODULE}] ${message}`);
  const el = globalThis.document?.getElementById?.("module-output");
  if (el) {
    el.value += `${message}\n`;
  }
}

// Resolve the WebSocket URL: the runner-injected global first, else the page location, else localhost.
function websocketUrl() {
  if (globalThis.__ET_WS_URL) {
    return globalThis.__ET_WS_URL;
  }
  const loc = globalThis.location;
  const proto = loc?.protocol === "https:" ? "wss:" : "ws:";
  const host = loc?.host ?? "localhost:8080";
  return `${proto}//${host}/ws`;
}

// Resolve the HTTP base the storage service is served from (runner-injected global, else page origin).
function httpBase() {
  if (globalThis.__ET_HTTP_BASE) {
    return globalThis.__ET_HTTP_BASE;
  }
  return globalThis.location?.origin ?? "http://localhost:8080";
}

// Open a WebSocket, complete the `et-connect` handshake, and resolve with { ws, agentId }.
//
// Storage's `put_file` rejects any bucket that is not a *currently connected* agent, so we must hold this
// socket open across the PUT/GET. The server assigns the id in its `et-connect-ack` reply.
function connectAgent() {
  const ws = new WebSocket(websocketUrl());
  return new Promise((resolve, reject) => {
    // `run`'s finally block only closes the socket it was handed, so a rejection here has to close its own --
    // otherwise a failed handshake leaves the runner holding an open socket the server keeps registered.
    // `settled` keeps a late ack from resolving a promise already rejected, which would hand back a closed ws.
    let settled = false;
    const fail = (message) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      ws.close();
      reject(new Error(message));
    };
    const timer = setTimeout(() => fail("timed out waiting for et-connect-ack"), 10000);
    ws.addEventListener("message", (event) => {
      let frame;
      try {
        frame = JSON.parse(event.data);
      } catch {
        return;
      }
      if (frame.type === "et-connect-ack" && frame.agent_id) {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve({ agentId: frame.agent_id, ws });
      }
    });
    ws.addEventListener("error", () => fail("websocket error before et-connect-ack"));
    ws.addEventListener("open", () => ws.send(JSON.stringify({ agent_id: null, type: "et-connect" })));
  });
}

// Build an S3 client aimed at et-storage-service.
//
// `endpoint` carries the `/storage` prefix and `forcePathStyle` makes the SDK address objects as
// `${endpoint}/${bucket}/${key}` -- i.e. `/storage/{agent_id}/{filename}`. The service is anonymous: it never
// validates the Authorization header, so we hand the SDK throwaway credentials (the resulting signature is
// ignored server-side) -- the most version-robust way to get v3 to emit the request. The checksum knobs are
// set to WHEN_REQUIRED to stop newer v3 defaults from wrapping the upload body in `aws-chunked` trailer
// framing, which the plain storage service would store verbatim and hand back on GET, breaking the round-trip.
function makeS3Client() {
  return new S3Client({
    credentials: { accessKeyId: "anonymous", secretAccessKey: "anonymous" },
    endpoint: `${httpBase()}/storage`,
    forcePathStyle: true,
    region: "us-east-1",
    requestChecksumCalculation: "WHEN_REQUIRED",
    responseChecksumValidation: "WHEN_REQUIRED",
  });
}

// Verify `HeadObject`: the storage service now serves HEAD with an ETag + Content-Length, so this is a
// checked step -- it throws (failing the run) if HEAD regresses or the ETag goes missing.
async function verifyHeadObject(s3, agentId) {
  const head = await s3.send(new HeadObjectCommand({ Bucket: agentId, Key: FILENAME }));
  log(`HeadObject ok: ContentLength=${head.ContentLength}, ETag=${head.ETag}`);
  if (head.ETag === undefined) {
    throw new Error("HeadObject returned no ETag");
  }
}

// Init hook. Nothing to boot for a pure-JS module; the runner still requires the default export to exist.
export default function init() {
  log("initialized");
}

// Execute the storage round-trip via the S3 client. Resolves on match; throws on mismatch or transport error.
export async function run() {
  log("entered run()");
  const { agentId, ws } = await connectAgent();
  log(`connected as ${agentId}`);

  const s3 = makeS3Client();
  const content = `Hello from ${MODULE} at ${new Date().toISOString()}!`;
  const body = new TextEncoder().encode(content);

  try {
    log(`PutObject -> bucket=${agentId} key=${FILENAME} (${body.length} bytes)`);
    const put = await s3.send(
      new PutObjectCommand({ Body: body, Bucket: agentId, ContentType: "text/plain", Key: FILENAME }),
    );
    if (put.ETag === undefined) {
      throw new Error("PutObject returned no ETag");
    }
    log(`PutObject ok: ETag=${put.ETag}`);

    log(`GetObject <- bucket=${agentId} key=${FILENAME}`);
    const got = await s3.send(new GetObjectCommand({ Bucket: agentId, Key: FILENAME }));
    const retrieved = await got.Body.transformToString();

    if (retrieved !== content) {
      log(`VERIFICATION FAILED: sent ${JSON.stringify(content)} but got ${JSON.stringify(retrieved)}`);
      throw new Error("data mismatch");
    }
    log("VERIFICATION SUCCESS - data matches!");

    await verifyHeadObject(s3, agentId);
  } finally {
    s3.destroy();
    ws.close();
  }
  log("workflow complete");
}
