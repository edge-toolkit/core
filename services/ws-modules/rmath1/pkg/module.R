# module.R -- the rmath1 FedAvg module. run() IS the control loop; the JS shim only boots webR, sets up the
# agent WebSocket transport, and hands control here.
#
# Storage-driven: run() waits for the broadcast math1-input pointer (captured by the shim onto
# globalThis.__etAgent.input), reads the input JSON (client datasets + hyperparameters) from ws-server
# storage with httr2 (tunnelled via the /websockify SOCKS5 relay), runs the FedAvg kernel -- only + - * /
# on doubles, bit-identical to the other math1 twins -- and PUTs the global model to math1-output.json in
# its own bucket, where the test harness reads and verifies it. httr2/jsonlite install at runtime from the
# webR package repo.

# --- transport: drive the shim's WsClient + browser globals via webr::eval_js (await = TRUE -> main thread) ---
js <- function(code) {
  webr::eval_js(code, await = TRUE)
}
agent_state <- function() as.character(js("globalThis.__etAgent.client.get_state()"))
agent_id <- function() as.character(js("globalThis.__etAgent.client.get_agent_id()"))
agent_disconnect <- function() js("globalThis.__etAgent.client.disconnect(); true")
agent_log <- function(msg) {
  js(sprintf("globalThis.__etAgent.log(%s); true", encodeString(msg, quote = "\"")))
}
agent_input_pointer <- function() {
  code <- paste0(
    "globalThis.__etAgent.input",
    " ? globalThis.__etAgent.input.bucket + '\\n' + globalThis.__etAgent.input.filename : ''"
  )
  as.character(js(code))
}
sleep_ms <- function(ms) {
  js(sprintf("new Promise(function(resolve){setTimeout(resolve, %d);})", as.integer(ms)))
}

# Point webR's Emscripten sockets at the /websockify relay so httr2/curl reach the server through it. This runs
# in the webR worker (no await = TRUE), where the SOCKFS filesystem lives.
rmath1_configure_socket <- function(relay_url) {
  code <- paste0(
    "SOCKFS.websocketArgs = SOCKFS.websocketArgs || {};",
    " SOCKFS.websocketArgs.url = ", encodeString(relay_url, quote = "\""), ";",
    " SOCKFS.websocketArgs.subprotocol = 'binary'; 0"
  )
  webr::eval_js(code)
}

# Run the FedAvg simulation on the parsed input and return the final global c(weight =, bias =).
# params$clients is a list of n x 2 matrices (feature, target); only + - * / on doubles in a fixed
# evaluation order, so the result is bit-identical to the other math1 language twins.
rmath1_fed_avg <- function(params) {
  clients <- params$clients
  rounds <- as.integer(params$rounds)
  epochs <- as.integer(params$epochs)
  learning_rate <- as.numeric(params$learning_rate)
  weight <- 0.0
  bias <- 0.0
  total_samples <- 0.0
  for (samples in clients) total_samples <- total_samples + as.numeric(nrow(samples))
  for (round in seq_len(rounds)) {
    merged_weight <- 0.0
    merged_bias <- 0.0
    for (samples in clients) {
      count <- as.numeric(nrow(samples))
      client_weight <- weight
      client_bias <- bias
      for (epoch in seq_len(epochs)) {
        grad_weight <- 0.0
        grad_bias <- 0.0
        for (i in seq_len(nrow(samples))) {
          feature <- samples[i, 1]
          target <- samples[i, 2]
          residual <- client_weight * feature + client_bias - target
          grad_weight <- grad_weight + residual * feature
          grad_bias <- grad_bias + residual
        }
        client_weight <- client_weight - learning_rate * (2.0 * grad_weight / count)
        client_bias <- client_bias - learning_rate * (2.0 * grad_bias / count)
      }
      merged_weight <- merged_weight + client_weight * count
      merged_bias <- merged_bias + client_bias * count
    }
    weight <- merged_weight / total_samples
    bias <- merged_bias / total_samples
  }
  c(weight = weight, bias = bias)
}

# The control loop the JS shim hands control to.
run <- function() {
  webr::install("httr2")
  webr::install("jsonlite")
  # Route curl through the /websockify relay's SOCKS5 front end (see the relay service). The proxy host is
  # nominal -- SOCKFS sends every socket to relay_url -- but the scheme must be socks5h so curl speaks SOCKS5.
  Sys.setenv(ALL_PROXY = "socks5h://127.0.0.1:8080")
  agent_log("rmath1: entered run()")

  # Point webR's sockets at this origin's /websockify relay (http(s) origin -> ws(s) relay URL).
  origin <- as.character(js("location.protocol + '//' + location.host"))
  relay_url <- sub("^http", "ws", paste0(origin, "/websockify"))
  rmath1_configure_socket(relay_url)

  # Wait for the agent WebSocket to connect and hand us the agent_id (this bucket).
  repeat {
    if (identical(agent_state(), "connected")) break
    sleep_ms(100)
  }
  bucket <- ""
  repeat {
    bucket <- agent_id()
    if (nzchar(bucket)) break
    sleep_ms(100)
  }
  agent_log(sprintf("rmath1: registered as %s", bucket))

  # Wait for the broadcast math1-input pointer ("bucket\nfilename" once captured by the shim).
  agent_log("rmath1: waiting for the math1-input pointer broadcast")
  pointer <- ""
  repeat {
    pointer <- agent_input_pointer()
    if (nzchar(pointer)) break
    sleep_ms(100)
  }
  parts <- strsplit(pointer, "\n", fixed = TRUE)[[1]]
  input_url <- sprintf("http://127.0.0.1:8080/storage/%s/%s", parts[[1]], parts[[2]])
  agent_log(sprintf("rmath1: reading input from %s", input_url))
  input_text <- httr2::request(input_url) |>
    httr2::req_perform() |>
    httr2::resp_body_string()
  params <- jsonlite::fromJSON(input_text, simplifyDataFrame = FALSE)

  agent_log(sprintf(
    "rmath1: running FedAvg - %d clients x %d rounds x %d local epochs",
    length(params$clients), as.integer(params$rounds), as.integer(params$epochs)
  ))
  model <- rmath1_fed_avg(params)
  agent_log(sprintf("rmath1: global model weight=%.17g bias=%.17g", model[["weight"]], model[["bias"]]))

  # %.17g preserves the exact f64 across the JSON round-trip the harness parses.
  output <- sprintf(
    "{\"module\":\"rmath1\",\"weight\":%.17g,\"bias\":%.17g}",
    model[["weight"]], model[["bias"]]
  )
  output_url <- sprintf("http://127.0.0.1:8080/storage/%s/math1-output.json", bucket)
  httr2::request(output_url) |>
    httr2::req_method("PUT") |>
    httr2::req_body_raw(output) |>
    httr2::req_perform()
  agent_log(sprintf("rmath1: stored the global model to %s", output_url))

  agent_disconnect()
  agent_log("rmath1: workflow complete")
}
