# WebSocket server

A Rust service using `actix-web` with WebSocket support.

**Features:**

- WebSocket endpoint at `/ws`
- Health check endpoint at `/health`
- OpenTelemetry tracing for all WebSocket operations
- JSON message protocol
- Connection lifecycle management
- Browser interface page at `/`
- Static WASM package `../ws-wasm-agent/pkg` served from `/pkg`.
- WASM workflow modules under `../ws-modules` served from `/modules`.
