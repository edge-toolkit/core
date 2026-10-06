# et-ws-wasm-agent

The browser-side edge-toolkit agent: a WASM client, built with wasm-bindgen, that connects a browser tab back to the
ws-server over WebSocket. [`WsClient`] registers the tab as an agent, keeps the connection alive with app-level
heartbeats, and exchanges the edge-toolkit wire protocol with the hub, configured through a [`WsClientConfig`].

The hub's root UI imports it from `/modules/@edge-toolkit/et-ws-wasm-agent/`, and browser ws-modules use it to talk
to the hub, so it is published both here and as an npm package built by `wasm-pack`.
