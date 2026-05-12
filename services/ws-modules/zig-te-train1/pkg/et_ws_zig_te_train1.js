// et_ws_zig_te_train1.js — main-thread loader and bridge for the Zig training agent.
// Runs the wasm in a Web Worker and proxies WebSocket + fetch via a SAB.
// Compared to zig-data1, adds a binary fetch path (request type 13) so the
// 49KB int8 image bytes can be copied directly into wasm memory, and adds an
// in-page training window so the user can interactively send train/infer
// envelopes without needing a peer agent.
//
// Shared memory layout (Int32 offsets at 0):
//   [0] signal: 0=idle, 1=request-pending
//   [1] request type: 0=sleep, 1=ws_connect, 2=ws_send, 3=ws_get_state,
//                     4=ws_get_agent_id, 5=ws_pop_response, 6=put_file,
//                     8=ws_disconnect, 9=log, 10=set_status, 11=get_ws_url,
//                     13=get_file_bin
//   [2] payload length (also response length)
//   [3] aux length (for put_file body)
// Data area starts at byte offset 16.

export default async function init() {}

let activeSession = null;

export function is_running() {
  return !!activeSession && !activeSession.done;
}

export function stop() {
  if (!activeSession || activeSession.done) return;
  activeSession.shutdown();
}

// run(options)
//   ui      (default true)  — build the floating trainer panel
//   onReady (default null)  — callback({ submit, shutdown }) invoked once
//                              the wasm is ready to receive envelopes. Useful
//                              for headless drivers (see pkg/demo/app.js).
//   onReply (default null)  — callback(rawPayload, parsedObj?) invoked for
//                              every wasm-originated reply envelope, in
//                              addition to the floating UI's own renderer.
//   waitUntilExit (default false) — resolve only after the worker exits.
//                              The ws-server host expects run() to resolve
//                              once startup completes so the Run/Stop button
//                              can be reused.
export const SUPPORTED_MODELS = ["mcunet", "mbv2", "proxyless"];

export async function run(options = {}) {
  if (is_running()) return activeSession.startPromise;

  const { ui: enableUi = true, onReady = null, onReply = null, waitUntilExit = false } = options;
  const model = activeModel(options.model);
  const DATA_OFFSET = 16;
  const sab = new SharedArrayBuffer(2 * 1024 * 1024);
  const ctrl = new Int32Array(sab, 0, 4);
  const data = new Uint8Array(sab, DATA_OFFSET);
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  const assetVersion = "training-memory-telemetry-20260525";
  const buildWasmUrl = (m) => {
    const o = new URL(`et_ws_zig_te_train1-${m}.wasm`, import.meta.url);
    o.searchParams.set("v", assetVersion);
    return o.href;
  };
  const workerUrlObj = new URL("et_ws_zig_te_train1_worker.js", import.meta.url);
  workerUrlObj.searchParams.set("v", assetVersion);
  const workerUrl = workerUrlObj.href;

  const respond = (bytesOrStr) => {
    if (bytesOrStr instanceof Uint8Array) {
      data.set(bytesOrStr);
      Atomics.store(ctrl, 2, bytesOrStr.length);
    } else if (bytesOrStr) {
      const b = enc.encode(bytesOrStr);
      data.set(b);
      Atomics.store(ctrl, 2, b.length);
    } else {
      Atomics.store(ctrl, 2, 0);
    }
    Atomics.store(ctrl, 0, 0);
    Atomics.notify(ctrl, 0);
  };

  let resolveStarted;
  let rejectStarted;
  const startPromise = new Promise((resolve, reject) => {
    resolveStarted = resolve;
    rejectStarted = reject;
  });
  let started = false;
  const completeStart = () => {
    if (started) return;
    started = true;
    resolveStarted();
  };
  const failStart = (error) => {
    if (started) return;
    started = true;
    rejectStarted(error);
  };
  const session = {
    done: false,
    startPromise,
    shutdown: () => {},
  };
  activeSession = session;

  const exitPromise = new Promise((resolve, reject) => {
    let ws = null,
      wsState = "disconnected",
      agentId = "",
      pending = [];
    let currentModel = model;
    // Worker handle for the *current* wasm instance. Reassigned each time
    // switchModel() respawns it; close button uses this to terminate the
    // active worker cleanly.
    let worker = null;
    // Latched true between the moment switchModel() pushes the shutdown
    // envelope and the moment the new worker boots. The worker.onmessage
    // hook reads this to decide whether `done:true` means "user closed the
    // panel — resolve run()" or "switching models — respawn quietly".
    let switching = false;
    let switchDone = null;
    // Set by the close button (or external shutdown()) so switchModel
    // bails out of respawning if the user clicked Close while a switch
    // was in flight. Without this guard, the close-shutdown envelope
    // gets dropped when switchModel clears pending, and the wasm session
    // silently restarts instead of exiting.
    let shutdownRequested = false;

    // ── Submit + shutdown helpers ──────────────────────────────────────────
    // These mutate the same `pending` queue the WebSocket loop drains, so
    // anything that calls submit({type:"train",...}) drives training the same
    // way the floating UI's buttons do. The demo page (pkg/demo/) uses these
    // directly via the `onReady` callback below.
    const submit = (envelope) => {
      // Catch close/shutdown so a switch-in-flight bails out cleanly.
      try {
        if (envelope && envelope.type === "shutdown") shutdownRequested = true;
      } catch (_) {}
      pending.push(JSON.stringify(envelope));
    };
    const shutdown = () => submit({ type: "shutdown" });
    session.shutdown = shutdown;

    // ── Model switch ───────────────────────────────────────────────────────
    // Replace the running wasm with a different backbone's .wasm in-place,
    // without reloading the host page. Keeps the floating UI, log scroll,
    // current canvas image, and JS-side state (heartbeat, etc.) alive.
    //
    // Sequence:
    //   1. Push shutdown to pending so the current wasm's message loop
    //      breaks cleanly (no need to .terminate() a worker mid-syscall).
    //   2. Wait for worker.onmessage to fire `done:true`; switching latch
    //      makes that handler call switchDone() instead of resolve(run()).
    //   3. Reset SAB control ints + pending queue + ws state so the new
    //      wasm boots into a clean handshake.
    //   4. Spawn a new worker with the new model's wasm URL.
    //
    // The poll() loop on the main thread keeps running across the swap —
    // it just observes no pending requests until the new wasm boots and
    // starts hitting the SAB again.
    async function switchModel(newModel) {
      if (switching) {
        // Already switching — sync the dropdown back to whatever is in
        // flight so the user doesn't see a stale value selected.
        try {
          ui.setModel(currentModel);
        } catch (_) {}
        return;
      }
      if (!SUPPORTED_MODELS.includes(newModel)) return;
      if (newModel === currentModel) return;
      switching = true;
      try {
        localStorage.setItem("te_train1_model", newModel);
      } catch (_) {}
      ui.log(`↻ switching to ${MODEL_LABELS[newModel] || newModel}…`);
      pending.push(JSON.stringify({ type: "shutdown" }));
      await new Promise((r) => {
        switchDone = r;
      });
      switchDone = null;
      // Drain stale state. Anything left in pending was queued for the
      // previous wasm; the new one would just log "unknown type" for them.
      pending.length = 0;
      Atomics.store(ctrl, 0, 0);
      Atomics.store(ctrl, 1, 0);
      Atomics.store(ctrl, 2, 0);
      Atomics.store(ctrl, 3, 0);
      try {
        ws?.close();
      } catch (_) {}
      ws = null;
      wsState = "disconnected";
      standaloneMode = false;
      agentId = "";
      onReadyFired = false;
      stopHeartbeat();
      currentModel = newModel;
      try {
        ui.setModel(newModel);
      } catch (_) {}
      switching = false;
      if (shutdownRequested) {
        // User clicked Close (or another submit({type:"shutdown"}) arrived)
        // mid-switch — don't respawn. resolve() with code 0 to match a
        // clean exit.
        resolve();
        return;
      }
      worker = spawnWorker(currentModel);
    }

    // ── Interactive training UI ────────────────────────────────────────────
    // Built only when options.ui is true (default). For headless drivers
    // (the demo page) we return a no-op ui shim so the rest of the bridge
    // can call ui.log / ui.handleReply unconditionally.
    const ui = enableUi
      ? buildTrainerUI({ onSubmit: submit, onSwitchModel: switchModel, model })
      : { log: () => {}, handleReply: () => {}, setModel: () => {} };

    // Hand the controller to the caller — demo drivers grab `submit`
    // from here once the wasm has finished its connect handshake.
    let onReadyFired = false;
    const fireOnReady = () => {
      if (onReadyFired) return;
      onReadyFired = true;
      if (onReady) {
        try {
          onReady({ submit, shutdown });
        } catch (e) {
          console.error(e);
        }
      }
      completeStart();
    };

    // ── Standalone-mode fallback ───────────────────────────────────────────
    // If no ws-server answers the connect within 3 s, fake a successful
    // handshake so the wasm enters its message loop and the UI can drive it.
    // js_ws_send becomes a no-op for the absent socket but still routes
    // replies to the UI tap in poll() case 2.
    //
    // `standaloneMode` latches once we commit so subsequent WS events
    // (the belated onclose from the dead socket, etc.) can't flip wsState
    // back to "disconnected" — which would otherwise cause the wasm's
    // wait_state() to time out after ~10 s and emit "agent exited cleanly"
    // mid-session.
    let standaloneMode = false;
    const enterStandalone = () => {
      if (standaloneMode) return;
      standaloneMode = true;
      wsState = "connected";
      agentId = "local-" + Math.random().toString(36).slice(2, 8);
      stopHeartbeat();
      ui.log("(no ws-server reachable — running standalone; agent_id=" + agentId + ")");
    };

    // ── Heartbeat ───────────────────────────────────────────────────────────
    // ws-server (services/ws/src/lib.rs:19) closes connections after
    // CONNECTION_TIMEOUT = 15 s of inactivity, where "activity" means any
    // inbound frame. The browser WebSocket API doesn't expose protocol-level
    // PING frames to JS, so we send the server's own `alive` envelope every
    // 10 s — that hits the `WsMessage::Alive { timestamp }` arm at
    // services/ws/src/lib.rs:271 which marks activity and replies with a
    // `response`. We filter those responses out in ws.onmessage below so the
    // wasm's message loop doesn't see them.
    let heartbeatTimer = null;
    const startHeartbeat = () => {
      if (heartbeatTimer) return;
      heartbeatTimer = setInterval(() => {
        if (standaloneMode) return;
        if (ws && ws.readyState === 1) {
          try {
            ws.send(JSON.stringify({ type: "alive", timestamp: new Date().toISOString() }));
          } catch {}
        }
      }, 10000);
    };
    const stopHeartbeat = () => {
      if (heartbeatTimer) {
        clearInterval(heartbeatTimer);
        heartbeatTimer = null;
      }
    };

    const poll = () => {
      if (Atomics.load(ctrl, 0) !== 1) {
        setTimeout(poll, 0);
        return;
      }

      const type = Atomics.load(ctrl, 1);
      const plen = Atomics.load(ctrl, 2);
      const alen = Atomics.load(ctrl, 3);
      const decStr = (off, len) => dec.decode(Uint8Array.from(data.subarray(off, off + len)));
      const payload = decStr(0, plen);
      const aux = alen ? decStr(plen, alen) : "";

      switch (type) {
        case 0:
          setTimeout(() => {
            respond();
            poll();
          }, parseInt(payload) || 0);
          return;
        case 1: {
          try {
            ws = new WebSocket(payload);
            wsState = "connecting";
          } catch {
            ws = null;
            setTimeout(enterStandalone, 0);
            respond();
            break;
          }
          // We deliberately do NOT call ws.close() before entering standalone.
          // Closing fires onclose synchronously-ish and we have to gate that
          // event with the standaloneMode latch to avoid clobbering wsState.
          const giveupTimer = setTimeout(() => {
            if (wsState !== "connected") enterStandalone();
          }, 3000);
          ws.onopen = () => {
            if (standaloneMode) return;
            clearTimeout(giveupTimer);
            wsState = "connected";
            try {
              ws.send(JSON.stringify({ type: "connect" }));
            } catch {}
            startHeartbeat();
          };
          ws.onmessage = (e) => {
            if (standaloneMode) return;
            try {
              const msg = JSON.parse(e.data);
              if (msg.type === "connect_ack" && msg.agent_id) {
                agentId = msg.agent_id;
              } else if (msg.type === "response") {
                // Server's reply to our `alive` keepalive. The inbound frame
                // already reset the server's inactivity timer; we don't need
                // to surface it to the wasm. Dropping it here keeps the
                // wasm's message loop from logging "unknown type: response"
                // every 10 s.
              } else {
                pending.push(typeof e.data === "string" ? e.data : "");
              }
            } catch {
              pending.push(typeof e.data === "string" ? e.data : "");
            }
          };
          ws.onclose = ws.onerror = () => {
            clearTimeout(giveupTimer);
            stopHeartbeat();
            if (standaloneMode) return; // wasm is in local mode; ignore
            if (wsState === "connecting") enterStandalone();
            else wsState = "disconnected"; // real connection genuinely dropped
          };
          respond();
          break;
        }
        case 2: {
          // Direct probe: dump every wasm-originated payload to console.
          console.log("[et-ws-zig-te-train1] ws_send:", payload);
          // Only forward protocol-level envelopes the server actually
          // understands (services/ws/src/lib.rs:246-446). The wasm's
          // {infer,train}_result / ready / error envelopes are LOCAL
          // responses to the UI driver — sending them to the server
          // produces "Received unrecognized message from client …" warnings.
          let outboundType = "";
          try {
            outboundType = JSON.parse(payload).type || "";
          } catch {}
          const forwardToServer = outboundType === "connect"
            || outboundType === "alive"
            || outboundType === "list_agents"
            || outboundType === "send_agent_message"
            || outboundType === "broadcast_message"
            || outboundType === "client_event"
            || outboundType === "store_file"
            || outboundType === "fetch_file";
          if (forwardToServer && ws && ws.readyState === 1) ws.send(payload);
          ui.handleReply(payload);
          // Tap the same payload to the external onReply (demo driver, etc.).
          // We also fire onReady on the first `ready` envelope so headless
          // callers can start submitting work as soon as the wasm is alive.
          if (onReply) {
            let parsed = null;
            try {
              parsed = JSON.parse(payload);
            } catch {}
            if (parsed && parsed.type === "ready") fireOnReady();
            try {
              onReply(payload, parsed);
            } catch (e) {
              console.error(e);
            }
          } else {
            try {
              if (JSON.parse(payload).type === "ready") fireOnReady();
            } catch {}
          }
          respond();
          break;
        }
        case 3:
          respond(wsState);
          break;
        case 4:
          respond(agentId);
          break;
        case 5: {
          const r = pending.shift() ?? "";
          respond(r);
          break;
        }
        case 6:
          fetch(payload, { method: "PUT", body: aux })
            .then(() => {
              respond();
              poll();
            })
            .catch(() => {
              respond();
              poll();
            });
          return;
        case 8:
          stopHeartbeat();
          try {
            ws?.close();
          } catch {}
          wsState = "disconnected";
          respond();
          break;
        case 9:
          console.log("[et-ws-zig-te-train1] log:", payload);
          appendOutput(payload);
          ui.log(payload);
          respond();
          break;
        case 10:
          console.log("[et-ws-zig-te-train1] status:", payload);
          appendOutput(payload);
          ui.log(payload);
          respond();
          break;
        case 11: {
          const p = location.protocol === "https:" ? "wss:" : "ws:";
          respond(`${p}//${location.host}/ws`);
          break;
        }
        case 13:
          fetch(payload)
            .then((r) => r.arrayBuffer())
            .then((b) => {
              respond(new Uint8Array(b));
              poll();
            })
            .catch(() => {
              respond();
              poll();
            });
          return;
        default:
          respond();
          break;
      }
      setTimeout(poll, 0);
    };

    const formatWorkerError = (info) => {
      // ErrorEvent fields (filename/lineno/colno/message/error) when fired by
      // worker.onerror; { message, name, stack } when we serialised it from
      // the worker's onmessage catch.
      if (info && info.message) {
        const where = info.filename
          ? ` @ ${info.filename}:${info.lineno ?? "?"}:${info.colno ?? "?"}`
          : "";
        return info.message + where;
      }
      if (info && info.error && info.error.message) return info.error.message;
      try {
        return JSON.stringify(info);
      } catch {
        return String(info);
      }
    };

    function spawnWorker(modelName) {
      const w = new Worker(workerUrl, { type: "classic" });
      w.onmessage = (e) => {
        if (!e.data.done) return;
        const wasSwitching = switching;
        if (e.data.error) {
          const msg = formatWorkerError(e.data.error);
          ui.log("! wasm worker exited with error: " + msg);
          console.error("[et-ws-zig-te-train1] worker error:", e.data.error);
        }
        w.terminate();
        if (wasSwitching) {
          // switchModel() is waiting on its `await new Promise…`.
          // Fire the continuation so it can boot the next wasm.
          if (switchDone) switchDone();
          return;
        }
        session.done = true;
        if (activeSession === session) activeSession = null;
        if (e.data.ret === 0) {
          completeStart();
          resolve();
        } else {
          const error = new Error(
            "et-ws-zig-te-train1: run() returned " + e.data.ret
              + (e.data.error ? " — " + formatWorkerError(e.data.error) : ""),
          );
          failStart(error);
          reject(error);
        }
      };
      w.onerror = (ev) => {
        const msg = formatWorkerError(ev);
        ui.log("! wasm worker error: " + msg);
        console.error("[et-ws-zig-te-train1] onerror:", ev);
        w.terminate();
        session.done = true;
        if (activeSession === session) activeSession = null;
        // Even during a switch we treat onerror as fatal — let the run()
        // promise reject so the host page sees something is wrong.
        if (switching && switchDone) switchDone();
        const error = new Error("et-ws-zig-te-train1 worker error: " + msg);
        failStart(error);
        reject(error);
      };
      w.postMessage({ sab, wasmUrl: buildWasmUrl(modelName) });
      return w;
    }

    worker = spawnWorker(currentModel);
    poll();
  });

  if (!waitUntilExit) {
    exitPromise.catch((error) => {
      console.error("[et-ws-zig-te-train1] worker exited after startup:", error);
    });
  }
  return waitUntilExit ? exitPromise : startPromise;
}

function appendOutput(msg) {
  const el = document.getElementById("module-output");
  if (el) el.value = (el.value ? el.value + "\n" : "") + msg;
}

// ── In-page trainer UI ───────────────────────────────────────────────────
// Builds a floating window (or, if the host page provides an element with
// id="module-ui-et-ws-zig-te-train1", attaches inside it). Lets the user:
//   - pick an image (webcam capture or file upload)
//   - center-crop + scale to 128×128
//   - choose a class label (0–9, matches the 10-class VWW codegen)
//   - send a train or infer envelope
//   - see wasm log lines and parsed train/infer replies
//
// All envelopes use blob: URLs so the wasm's `js_get_file_bin` round-trips
// through main-thread fetch() without leaving the page.
export const MODEL_LABELS = {
  mcunet: "mcunet-5fps",
  mbv2: "mbv2-w0.35",
  proxyless: "proxyless-w0.3",
};

/**
 * Returns the model name the next run() call will instantiate, mirroring
 * the internal resolveModel() precedence: explicit arg > localStorage >
 * "mcunet" default. Exported so callers in the same package (e.g. the
 * demo page's model indicator) can display the live selection without
 * re-implementing the same logic and drifting.
 */
export function activeModel(requested) {
  if (requested && SUPPORTED_MODELS.includes(requested)) return requested;
  try {
    const stored = globalThis.localStorage && localStorage.getItem("te_train1_model");
    if (stored && SUPPORTED_MODELS.includes(stored)) return stored;
  } catch (_) {}
  return "mcunet";
}

function buildTrainerUI({ onSubmit, onSwitchModel, model }) {
  if (typeof document === "undefined") {
    // Non-browser environment (test runner, etc.) — return no-op stubs.
    return { log: () => {}, handleReply: () => {}, setModel: () => {} };
  }

  // Injected once per page load — scoped to #module-ui-et-ws-zig-te-train1
  // so it can't bleed into the host page. The media query handles the
  // small-viewport case (phones, narrow split-screens) by anchoring the
  // panel to all four edges and bumping touch-target sizes; the desktop
  // default keeps the original floating 360 px card.
  if (!document.getElementById("module-ui-et-ws-zig-te-train1-style")) {
    const style = document.createElement("style");
    style.id = "module-ui-et-ws-zig-te-train1-style";
    style.textContent = `
      #module-ui-et-ws-zig-te-train1 button {
        min-height: 34px;
        font-size: 13px;
        padding: 6px 12px;
        border-radius: 4px;
        border: none;
        cursor: pointer;
        /* Default neutral background; the action buttons override via
         * their inline style attributes (green for train, blue for infer,
         * etc.). Without this fallback the cam-toggle / file-pick buttons
         * would inherit the host page's button style. */
        background: #333;
        color: #eee;
      }
      #module-ui-et-ws-zig-te-train1 button:disabled {
        opacity: 0.5;
        cursor: not-allowed;
      }
      #module-ui-et-ws-zig-te-train1 select {
        min-height: 34px;
        font-size: 13px;
        padding: 4px 8px;
      }
      #module-ui-et-ws-zig-te-train1 [data-role="title"] button {
        min-height: 32px;
      }
      @media (max-width: 480px) {
        #module-ui-et-ws-zig-te-train1 {
          top: 8px !important;
          right: 8px !important;
          left: 8px !important;
          width: auto !important;
          max-height: calc(100vh - 16px);
          overflow-y: auto;
          -webkit-overflow-scrolling: touch;
        }
        #module-ui-et-ws-zig-te-train1 button,
        #module-ui-et-ws-zig-te-train1 select {
          min-height: 40px;
          font-size: 14px;
        }
        #module-ui-et-ws-zig-te-train1 button {
          padding: 8px 14px;
        }
        #module-ui-et-ws-zig-te-train1 [data-role="title-chip"] {
          font-size: 11px;
        }
        #module-ui-et-ws-zig-te-train1 pre {
          font-size: 12px !important;
          height: 200px !important;
        }
        /* Drop the canvas/description side-by-side layout; stack on mobile
         * so the canvas keeps its native size and the description doesn't
         * shrink to one word per line beside it. */
        #module-ui-et-ws-zig-te-train1 [data-role="canvas-row"] {
          flex-direction: column;
          align-items: flex-start !important;
        }
      }
    `;
    document.head.appendChild(style);
  }

  let host = document.getElementById("module-ui-et-ws-zig-te-train1");
  if (!host) {
    host = document.createElement("div");
    host.id = "module-ui-et-ws-zig-te-train1";
    host.style.cssText = [
      "position:fixed",
      "top:16px",
      "right:16px",
      // min() keeps the panel readable on desktop (360 px) while letting it
      // shrink on narrower viewports (laptop split-view, foldables) without
      // overflowing. The mobile media query above takes over below 480 px.
      "width:min(360px, calc(100vw - 32px))",
      "max-height:calc(100vh - 32px)",
      "overflow-y:auto",
      "background:#1e1e1e",
      "color:#eee",
      "font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif",
      "padding:12px",
      "border-radius:8px",
      "box-shadow:0 4px 16px rgba(0,0,0,0.35)",
      "z-index:99999",
      "font-size:13px",
      "line-height:1.4",
    ].join(";");
    document.body.appendChild(host);
  }

  const modelOptions = ["mcunet", "mbv2", "proxyless"]
    .map((m) => `<option value="${m}" ${m === model ? "selected" : ""}>${MODEL_LABELS[m]}</option>`)
    .join("");
  // Title is split into 3 rows so each survives a narrow viewport:
  //   row 1: heading + close button (always reachable with one thumb)
  //   row 2: model dropdown + demo opener (the two controls that change scope)
  //   row 3: info chip — gets updated by the memory reply with kb counts
  // `touch-action:none` on the title row stops mobile browsers from
  // interpreting the drag-gesture as page scroll mid-drag.
  host.innerHTML = `
    <div data-role="title" style="display:flex;justify-content:space-between;align-items:center;gap:6px;margin-bottom:6px;cursor:move;user-select:none;touch-action:none;">
      <strong style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">TinyEngine VWW training</strong>
      <button data-act="close" title="Close training window" style="background:transparent;color:#bbb;font-size:18px;line-height:1;padding:4px 10px;min-width:36px;">✕</button>
    </div>
    <div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;flex-wrap:wrap;">
      <select data-role="model" title="Backbone (changes reload the page)" style="background:#111;color:#eee;border:1px solid #444;border-radius:4px;flex:1;min-width:140px;">${modelOptions}</select>
      <button data-act="demo" title="Open synthetic-dataset training demo" style="background:#2a4a6d;color:#fff;">📚 Demo</button>
    </div>
    <div data-role="title-chip" style="font-size:11px;color:#888;margin-bottom:8px;word-wrap:break-word;">${
    MODEL_LABELS[model]
  } · 128×128 · prototype head</div>
    <div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;flex-wrap:wrap;">
      <button data-act="cam-toggle">📷 Start camera</button>
      <button data-act="cam-flip" hidden>🔄 Flip</button>
      <button data-act="file-pick">📁 File…</button>
      <input type="file" accept="image/*" hidden>
    </div>
    <video autoplay muted playsinline style="width:100%;max-height:200px;background:#000;border-radius:4px;display:none;margin-bottom:6px;"></video>
    <div data-role="canvas-row" style="display:flex;gap:8px;align-items:center;margin-bottom:6px;">
      <canvas width="128" height="128" style="background:#000;border-radius:4px;image-rendering:pixelated;border:1px solid #333;width:192px;height:192px;flex:none;"></canvas>
      <div style="font-size:11px;color:#aaa;">128×128 RGB input<br>(model expects int8 = pixel−128)</div>
    </div>
    <!-- Capture sits next to Train/Infer on mobile: tapping Capture then
         immediately Train/Infer is the common flow, so keeping them in one
         row removes the scroll/jump between camera-controls and action
         buttons that previously hurt one-handed phone use. -->
    <div style="display:flex;gap:6px;align-items:center;margin-bottom:6px;flex-wrap:wrap;">
      <button data-act="cam-snap" disabled style="background:#444;color:#fff;">📸 Capture</button>
      <label style="display:flex;align-items:center;gap:4px;">Label
        <select style="background:#111;color:#eee;border:1px solid #444;border-radius:4px;">
          <option value="0">0 · scene (no person)</option>
          <option value="1">1 · person</option>
        </select>
      </label>
      <button data-act="train" disabled style="background:#2a5d2a;color:#fff;">Train</button>
      <button data-act="infer" disabled style="background:#2a4a6d;color:#fff;">Infer</button>
    </div>
    <pre style="background:#111;color:#9f9;font-family:ui-monospace,monospace;font-size:11px;padding:8px;border-radius:4px;height:160px;overflow-y:auto;margin:0;white-space:pre-wrap;word-break:break-word;-webkit-overflow-scrolling:touch;"></pre>
  `;

  const $ = (sel) => host.querySelector(sel);
  const titleBar = $("[data-role=\"title\"]");
  const video = $("video");
  const canvas = $("canvas");
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  const fileInput = $("input[type=file]");
  /* The first `<select>` is the model dropdown in the title bar — query by
   * label-section ancestor to get the label dropdown instead. */
  const selectEl = host.querySelector("label select");
  const modelEl = host.querySelector("select[data-role=\"model\"]");
  const logEl = $("pre");
  const btn = (act) => $(`button[data-act="${act}"]`);

  // ── Model selector ────────────────────────────────────────────────────────
  // Dropdown change triggers in-place switching: the run() closure
  // pushes a shutdown envelope, waits for the wasm to exit, then spawns a
  // new worker with the selected model's wasm. The UI panel (this DOM),
  // the log, the canvas image — all persist across the swap. If
  // onSwitchModel isn't provided (older callers) fall back to a reload.
  modelEl.addEventListener("change", () => {
    const v = modelEl.value;
    if (onSwitchModel) {
      onSwitchModel(v);
    } else {
      try {
        localStorage.setItem("te_train1_model", v);
      } catch (_) {}
      location.reload();
    }
  });

  // Called by run() after a successful switch so we can re-render the
  // model name in the title chip and re-sync the dropdown (e.g. if the
  // switch was triggered programmatically). Keeps the chip readable even
  // before the new wasm reports its memory budget.
  function setModel(newModel) {
    if (modelEl.value !== newModel) modelEl.value = newModel;
    const chip = host.querySelector("[data-role=\"title-chip\"]");
    if (chip) {
      chip.textContent = `${MODEL_LABELS[newModel] || newModel} · 128×128 · prototype head`;
    }
  }

  let hasImage = false;
  const setReady = (r) => {
    btn("train").disabled = !r;
    btn("infer").disabled = !r;
  };

  // ── Camera (toggleable, prefers rear-facing) ───────────────────────────
  // facingMode "environment" = rear camera on phones; "user" = front-facing.
  // {ideal: ...} is a soft preference, so laptops with a single webcam still
  // match. The flip button swaps the value and restarts the stream.
  let stream = null;
  let facingMode = "environment";

  async function startCamera() {
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: { ideal: facingMode } },
      });
      video.srcObject = stream;
      video.style.display = "block";
      btn("cam-snap").disabled = false;
      btn("cam-flip").hidden = false;
      btn("cam-toggle").textContent = "🛑 Stop camera";
    } catch (e) {
      uiLog("camera: " + e.message);
    }
  }

  function stopCamera() {
    if (stream) {
      for (const track of stream.getTracks()) track.stop();
      stream = null;
    }
    video.srcObject = null;
    video.style.display = "none";
    btn("cam-snap").disabled = true;
    btn("cam-flip").hidden = true;
    btn("cam-toggle").textContent = "📷 Start camera";
  }

  btn("cam-toggle").addEventListener("click", () => {
    if (stream) stopCamera();
    else startCamera();
  });

  btn("cam-flip").addEventListener("click", async () => {
    facingMode = facingMode === "environment" ? "user" : "environment";
    stopCamera();
    await startCamera();
  });

  btn("cam-snap").addEventListener("click", () => {
    drawToCanvas(video, video.videoWidth, video.videoHeight);
    hasImage = true;
    setReady(true);
  });

  // ── Close / exit training ──────────────────────────────────────────────
  // Stop the camera, push a shutdown envelope so the wasm exits cleanly
  // (its main loop breaks → js_ws_disconnect → returns 0 → worker terminates
  // → the run() promise resolves), then remove the floating window from the
  // DOM. The host page is left with no trace beyond the host's #module-output
  // element if it had one.
  btn("close").addEventListener("click", () => {
    stopCamera();
    try {
      onSubmit({ type: "shutdown" });
    } catch {}
    host.remove();
  });

  btn("demo").addEventListener("click", () => {
    // Resolve demo URL relative to this script's package. import.meta.url
    // points at et_ws_zig_te_train1.js; the demo lives at ./demo/index.html
    // alongside it, served by et-modules-service from this module's pkg/.
    const demoUrl = new URL("./demo/index.html", import.meta.url).href;
    window.open(demoUrl, "et-ws-zig-te-train1-demo", "width=960,height=720");
  });

  // ── Drag (title bar acts as a window handle) ───────────────────────────
  // pointerdown on the title row pins the host via left/top (overriding the
  // initial right:16px anchor) and starts tracking. pointermove updates the
  // position, clamped to the viewport. Buttons inside the title bar are
  // exempted so the close click still works. setPointerCapture keeps move
  // events flowing even when the pointer leaves the title bar mid-drag.
  let dragging = false;
  let dragDX = 0;
  let dragDY = 0;
  titleBar.addEventListener("pointerdown", (e) => {
    if (e.target.closest("button")) return;
    const rect = host.getBoundingClientRect();
    host.style.position = "fixed";
    host.style.left = rect.left + "px";
    host.style.top = rect.top + "px";
    host.style.right = "auto";
    dragDX = e.clientX - rect.left;
    dragDY = e.clientY - rect.top;
    dragging = true;
    try {
      titleBar.setPointerCapture(e.pointerId);
    } catch {}
  });
  titleBar.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const w = host.offsetWidth;
    const h = host.offsetHeight;
    const x = Math.max(0, Math.min(window.innerWidth - w, e.clientX - dragDX));
    const y = Math.max(0, Math.min(window.innerHeight - h, e.clientY - dragDY));
    host.style.left = x + "px";
    host.style.top = y + "px";
  });
  const endDrag = (e) => {
    if (!dragging) return;
    dragging = false;
    try {
      titleBar.releasePointerCapture(e.pointerId);
    } catch {}
  };
  titleBar.addEventListener("pointerup", endDrag);
  titleBar.addEventListener("pointercancel", endDrag);

  btn("file-pick").addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", () => {
    const file = fileInput.files[0];
    if (!file) return;
    const img = new Image();
    img.onload = () => {
      drawToCanvas(img, img.naturalWidth, img.naturalHeight);
      hasImage = true;
      setReady(true);
      URL.revokeObjectURL(img.src);
    };
    img.src = URL.createObjectURL(file);
  });

  // Path C: the regenerated 49KB sparse-bp codegen (mcunet-5fps backbone, see
  // tools/smoke_pathc.mjs for end-to-end verification) takes 128×128×3 int8
  // input at &buffer0[65536]. The canvas storage is the native 128×128 — CSS
  // scales it up to 192px on screen for visibility but the pixel data we
  // sample is the raw 128×128. Each train envelope fires BOTH te_train_step
  // (on-graph QAS sparse-update over the 25 v8..v15_* tensors) AND the fp32
  // prototype binary head (te_train_binary_step accumulates class-mean
  // feature vectors). The infer reply's binary_argmax reflects the prototype
  // head's nearest-centroid pick.
  const INPUT_W = 128;
  const INPUT_H = 128;

  function drawToCanvas(src, w, h) {
    if (!w || !h) return;
    const side = Math.min(w, h);
    const sx = (w - side) / 2;
    const sy = (h - side) / 2;
    ctx.drawImage(src, sx, sy, side, side, 0, 0, INPUT_W, INPUT_H);
  }

  // Build a 49152-byte signed-int8 NHWC RGB tensor from the canvas (128*128*3).
  // Each pixel emits (r-128, g-128, b-128), matching the tutorial firmware's
  // camera-frame normalisation at tutorial/training/Src/main.cpp:108-122.
  function canvasToInt8Blob() {
    const pixels = ctx.getImageData(0, 0, INPUT_W, INPUT_H).data;
    const buf = new Int8Array(INPUT_W * INPUT_H * 3);
    let j = 0;
    for (let i = 0; i < INPUT_W * INPUT_H; i++) {
      buf[j++] = pixels[i * 4 + 0] - 128;
      buf[j++] = pixels[i * 4 + 1] - 128;
      buf[j++] = pixels[i * 4 + 2] - 128;
    }
    return new Blob([buf], { type: "application/octet-stream" });
  }

  function dispatch(envelopeType) {
    if (!hasImage) return;
    const url = URL.createObjectURL(canvasToInt8Blob());
    const env = envelopeType === "train"
      ? { type: "train", url, label: parseInt(selectEl.value, 10) }
      : { type: "infer", url };
    onSubmit(env);
    uiLog(
      envelopeType === "train"
        ? `→ train (label=${env.label})`
        : `→ infer · running forward pass…`,
    );
    // Revoke after a generous timeout so the wasm has time to fetch.
    setTimeout(() => URL.revokeObjectURL(url), 30000);
  }

  btn("train").addEventListener("click", () => dispatch("train"));
  btn("infer").addEventListener("click", () => dispatch("infer"));

  function uiLog(msg) {
    if (!msg) return;
    logEl.textContent += msg + "\n";
    logEl.scrollTop = logEl.scrollHeight;
  }

  // VWW class-name lookup for the binary head. Matches the demo's
  // SCENE=0 / PERSON=1 convention (MIT VWW labelling: class 0 is the
  // "no person" / background class, class 1 is "person").
  const CLASS_NAMES = ["scene", "person"];
  const labelName = (i) => CLASS_NAMES[i] !== undefined ? `${i} (${CLASS_NAMES[i]})` : `${i}`;

  function handleReply(payload) {
    try {
      const obj = JSON.parse(payload);
      if (obj.type === "infer_result") {
        // Path C reply has scores[10] (codegen's mcunet-5fps int8 head, often
        // saturated at 0 across all 10 slots → softmax-of-logits is meaningless
        // there), plus binary_scores[10] from the fp32 prototype head (only
        // slots 0/1 carry signal; 2..9 are zero-padded). For the user-facing
        // confidence display we softmax over JUST the two binary slots — that
        // matches the only task the prototype head is actually classifying
        // (scene vs person). The built-in head's argmax is reported alongside
        // as an alternate pick in case the backbone alone is discriminative.
        const pick = obj.binary_argmax !== undefined ? obj.binary_argmax : obj.argmax;
        const bs = obj.binary_scores;
        let conf = NaN;
        if (bs && bs.length >= 2) {
          const lo = Math.min(bs[0], bs[1]);
          const e0 = Math.exp((bs[0] - lo) / 1024);
          const e1 = Math.exp((bs[1] - lo) / 1024);
          conf = (pick === 0 ? e0 : e1) / (e0 + e1) * 100;
        }
        const altPick = pick === 0 ? 1 : 0;
        const builtinHead = obj.argmax !== undefined
          ? ` · built-in head→${labelName(obj.argmax)}`
          : "";
        uiLog(
          `← predicted ${labelName(pick)}`
            + (isFinite(conf) ? ` · ${conf.toFixed(1)}% confidence` : "")
            + ` · alt: ${labelName(altPick)}`
            + builtinHead,
        );
      } else if (obj.type === "train_result") {
        uiLog(`← train done (label=${labelName(obj.label)})`);
      } else if (obj.type === "ready") {
        uiLog(`← ready: ${obj.classes} classes, ${obj.input_w}×${obj.input_h}×${obj.input_c}`);
        // Now that the wasm is ready, fetch the memory budget and surface
        // it in the title chip + log. This is the paper's "peak SRAM /
        // model size" reporting — computed by the codegen's static
        // GeneralMemoryScheduler analysis (PEAK_MEM, MODEL_SIZE), plus
        // our binary-head wrapper's runtime overhead.
        onSubmit({ type: "get_memory" });
      } else if (obj.type === "memory") {
        const kb = (b) => (b / 1024).toFixed(1);
        // Update the title-chip text. Matches the MCUNetV3 paper's
        // headline format ("256 KB SRAM / 1 MB flash") so a reader
        // familiar with the paper can compare at a glance.
        const chip = host.querySelector("[data-role=\"title-chip\"]");
        if (chip) {
          chip.textContent = `${MODEL_LABELS[model]} · 128×128 · prototype head · ${
            kb(obj.train_sram_peak)
          } KB SRAM · ${kb(obj.model_flash)} KB flash`;
        }
        uiLog(
          `← memory: peak_SRAM=${kb(obj.peak_sram)} KB, `
            + `flash=${kb(obj.model_flash)} KB, `
            + `binary_head=${obj.binary_head} B, `
            + `train_SRAM=${kb(obj.train_sram_peak)} KB`,
        );

        // Paper Figure 10 comparison panel. Show the three variants as
        // KB with the savings ratio so the reader sees the impact of the
        // reorder optimization at a glance. The wasm only LINKS the
        // FT-SU+R variant (ft_sur_sram == peak_sram); the other two are
        // analytical bounds for the same model, reported for context.
        if (obj.ft_full_sram !== undefined && obj.ft_sur_sram > 0) {
          const ratioFull = (obj.ft_full_sram / obj.ft_sur_sram).toFixed(1);
          const ratioSU = (obj.ft_su_sram / obj.ft_sur_sram).toFixed(1);
          let panel = host.querySelector("[data-role=\"fig10-panel\"]");
          if (!panel) {
            panel = document.createElement("div");
            panel.dataset.role = "fig10-panel";
            panel.style.cssText = "font-size:10px;color:#888;background:#161616;border:1px solid #2a2a2a;"
              + "border-radius:4px;padding:6px 8px;margin:0 0 8px 0;"
              + "font-family:ui-monospace,monospace;white-space:nowrap;overflow-x:auto;";
            // Insert immediately after the title chip so it tracks the
            // backbone selection.
            chip.parentNode.insertBefore(panel, chip.nextSibling);
          }
          panel.innerHTML = `<span style="color:#aaa;">paper Fig.10 peak SRAM</span><br>`
            + `&nbsp;&nbsp;FT-Full = ${kb(obj.ft_full_sram).padStart(7)} KB &nbsp;(${ratioFull}× SU+R)<br>`
            + `&nbsp;&nbsp;FT-SU&nbsp;&nbsp; = ${kb(obj.ft_su_sram).padStart(7)} KB &nbsp;(${ratioSU}× SU+R)<br>`
            + `&nbsp;&nbsp;FT-SU+R = ${
              kb(obj.ft_sur_sram).padStart(7)
            } KB &nbsp;<span style="color:#6fc06f;">← linked</span>`;
          uiLog(
            `← fig10: FT-Full=${kb(obj.ft_full_sram)} KB, `
              + `FT-SU=${kb(obj.ft_su_sram)} KB, `
              + `FT-SU+R=${kb(obj.ft_sur_sram)} KB `
              + `(${ratioFull}× / ${ratioSU}× reduction)`,
          );
        }
      } else if (obj.type === "error") {
        uiLog(`! ${obj.message}`);
      } else {
        uiLog(`← ${payload}`);
      }
    } catch {
      uiLog(`← ${payload}`);
    }
  }

  return { log: uiLog, handleReply, setModel };
}
