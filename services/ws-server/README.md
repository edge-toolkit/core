# WebSocket server

A Rust service using `actix-web` with WebSocket support.

**Features:**

- WebSocket endpoint at `/ws`
- Health check endpoint at `/health`
- OpenTelemetry tracing for all WebSocket operations
- JSON message protocol
- Connection lifecycle management
- Browser demo page at `/`
- Static WASM package `../ws-wasm-agent` served from `/pkg`.
