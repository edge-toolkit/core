# et-ws-service

The agent registry and WebSocket hub at the centre of an edge-toolkit deployment. [`configure`] mounts `/ws` on an
`actix-web` app; every agent that connects there is assigned an id, tracked in a [`WsAgentRegistry`], and can send
messages to one other agent or to all of them. Direct messages to an agent that is offline are queued until it
reconnects. A lone hub persists the registry as YAML so ids and queues survive a restart; hubs scaled out behind a
load balancer share it over the NATS backplane that `WsConfig::backplane` names instead.

[`configure`] also mounts the admin API, which `WsConfig::admin_token` guards. `PUT /blocked-ips/{ip}` blocks an IP
address, and `DELETE /agents/{agent_id}` evicts an agent by blocking the address it last connected from and dropping
the messages it queued. The registry's blocklist closes a blocked address's sockets, refuses its upgrades, and is
what the storage routes consult to refuse it too. The blocklist persists with the
registry, or in the backplane's shared blocklist.

The hub routes the messages of the edge-toolkit wire protocol, whose Rust definitions are `ClientMessage` and
`ServerMessage` in the `edge-toolkit` crate. Frames it does not recognise -- any text it cannot parse as a known
`ClientMessage`, and any binary frame -- are forwarded verbatim to every other connected agent, with a single `info!`
log per broadcast. This lets agents use arbitrary out-of-band payloads without needing a server-side enum entry. An
explicit `et-broadcast-message` still wraps its payload in an `et-agent-message` envelope. Both paths require the
sender to be a connected agent; frames from unassigned clients are dropped.
