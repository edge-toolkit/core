# module.R -- the rcomm1 communication module. run() IS the control loop; the JS shim only boots webR, sets up
# the agent WebSocket transport, and hands control here.
#
# Mirrors comm1 / dart-comm1: connect, discover a peer via et-list-agents, broadcast a message to every other
# agent, then send a direct message to the peer. R drives the JS-side WsClient through webr::eval_js (webR
# cannot open the agent WebSocket itself) and composes every ClientMessage with jsonlite (installed at runtime;
# base R has no JSON tooling). The message payloads match comm1's exactly (module / step / from_agent_id /
# message), and the timing matches comm1's (100 ms connect poll, 1 s list-agents poll, 3 s message pauses).

# --- transport: drive the shim's WsClient via webr::eval_js (await = TRUE -> main thread) ---
js <- function(code) {
  webr::eval_js(code, await = TRUE)
}
agent_state <- function() as.character(js("globalThis.__etAgent.client.get_state()"))
agent_id <- function() as.character(js("globalThis.__etAgent.client.get_agent_id()"))
agent_last_agents <- function() as.character(js("globalThis.__etAgent.lastAgents"))
agent_disconnect <- function() js("globalThis.__etAgent.client.disconnect(); true")
agent_send <- function(message) {
  js(sprintf("globalThis.__etAgent.client.send(%s); true", encodeString(message, quote = "\"")))
}
agent_log <- function(msg) {
  js(sprintf("globalThis.__etAgent.log(%s); true", encodeString(msg, quote = "\"")))
}
sleep_ms <- function(ms) {
  js(sprintf("new Promise(function(resolve){setTimeout(resolve, %d);})", as.integer(ms)))
}

# --- messages (matching comm1's payload shapes) ---

# The et-list-agents request carries no fields.
rcomm1_list_agents <- function() {
  '{"type":"et-list-agents"}'
}

# Compose the broadcast ClientMessage (et-broadcast-message).
rcomm1_broadcast <- function(self_id) {
  message <- list(
    module = "rcomm1",
    step = "broadcast",
    from_agent_id = self_id,
    message = "rcomm1 broadcast to all other connected agents"
  )
  as.character(jsonlite::toJSON(list(type = "et-broadcast-message", message = message), auto_unbox = TRUE))
}

# Compose the direct ClientMessage (et-send-agent-message) to `peer_id`.
rcomm1_direct <- function(self_id, peer_id) {
  message <- list(
    module = "rcomm1",
    step = "direct",
    from_agent_id = self_id,
    message = "rcomm1 direct message"
  )
  envelope <- list(type = "et-send-agent-message", to_agent_id = peer_id, message = message)
  as.character(jsonlite::toJSON(envelope, auto_unbox = TRUE))
}

# Pick the target peer from an et-list-agents-response: the first connected agent that is not this agent.
# Returns "" when no peer is present yet, so the poll loop keeps trying (as comm1 / dart-comm1 do).
rcomm1_pick_peer <- function(response_json, self_id) {
  parsed <- tryCatch(jsonlite::fromJSON(response_json, simplifyVector = FALSE), error = function(e) NULL)
  if (is.null(parsed) || !identical(parsed$type, "et-list-agents-response")) {
    return("")
  }
  for (agent in parsed$agents) {
    connected <- identical(agent$state, "connected")
    if (connected && !is.null(agent$agent_id) && !identical(agent$agent_id, self_id)) {
      return(agent$agent_id)
    }
  }
  ""
}

# The control loop the JS shim hands control to.
run <- function() {
  webr::install("jsonlite")
  agent_log("rcomm1: entered run()")

  # Wait for the agent WebSocket to connect and hand us our agent_id.
  repeat {
    if (identical(agent_state(), "connected")) break
    sleep_ms(100)
  }
  self_id <- ""
  repeat {
    self_id <- agent_id()
    if (nzchar(self_id)) break
    sleep_ms(100)
  }
  agent_log(sprintf("rcomm1: websocket connected with agent_id=%s", self_id))

  # Poll for a peer, exactly like comm1 -- ask the server for the agent list until R finds a connected peer.
  peer <- ""
  repeat {
    agent_send(rcomm1_list_agents())
    sleep_ms(1000)
    peer <- rcomm1_pick_peer(agent_last_agents(), self_id)
    if (nzchar(peer)) break
  }
  agent_log(sprintf("rcomm1: found connected peer agent %s; sending broadcast", peer))

  agent_send(rcomm1_broadcast(self_id))
  sleep_ms(3000)
  agent_log(sprintf("rcomm1: sending direct message to %s", peer))
  agent_send(rcomm1_direct(self_id, peer))
  sleep_ms(3000)
  agent_disconnect()
  agent_log("rcomm1: workflow complete")
}
