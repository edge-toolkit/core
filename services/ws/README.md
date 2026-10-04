# et-ws-service

The agent registry and WebSocket hub at the centre of an edge-toolkit deployment. [`configure`] mounts `/ws` on an
`actix-web` app; every agent that connects there is assigned an id, tracked in a [`WsAgentRegistry`], and can send
messages to one other agent or to all of them. Direct messages to an agent that is offline are queued until it
reconnects, and the registry is persisted as YAML so ids and queues survive a restart.

The hub routes the messages of the edge-toolkit wire protocol, whose Rust definitions are `ClientMessage` and
`ServerMessage` in the `edge-toolkit` crate. Frames it does not recognise -- any text it cannot parse as a known
`ClientMessage`, and any binary frame -- are forwarded verbatim to every other connected agent, with a single `info!`
log per broadcast. This lets agents use arbitrary out-of-band payloads without needing a server-side enum entry. An
explicit `et-broadcast-message` still wraps its payload in an `et-agent-message` envelope. Both paths require the
sender to be a connected agent; frames from unassigned clients are dropped.
