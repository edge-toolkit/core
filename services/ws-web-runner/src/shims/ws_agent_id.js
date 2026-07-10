// Capture the server-assigned agent_id from the et-connect-ack frame, for the coverage test only.
// Browser wasm has no filesystem, so a module's coverage is PUT to ws-server storage -- but put_file only accepts
// a bucket that is a registered agent. The module's own agent_id IS registered, so the runner PUTs there; this
// shim wraps globalThis.WebSocket to observe the inbound et-connect-ack and stash agent_id on globalThis. Inert
// unless __ET_TEST_COVERAGE is set, so shipped runs keep the native WebSocket untouched.
if (globalThis.__ET_TEST_COVERAGE && typeof globalThis.WebSocket === "function") {
  const NativeWebSocket = globalThis.WebSocket;
  globalThis.WebSocket = class extends NativeWebSocket {
    constructor(...args) {
      super(...args);
      this.addEventListener("message", (event) => {
        if (typeof event.data !== "string") {
          return;
        }
        try {
          const message = JSON.parse(event.data);
          if (message && message.type === "et-connect-ack" && typeof message.agent_id === "string") {
            globalThis.__ET_AGENT_ID = message.agent_id;
          }
        } catch (_error) {
          // Non-JSON or non-ack frames are irrelevant to agent-id capture.
        }
      });
    }
  };
}
