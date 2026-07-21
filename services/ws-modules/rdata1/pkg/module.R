# module.R -- the rdata1 data module. run() IS the control loop; the JS shim only boots webR, sets up the agent
# WebSocket transport, and hands control here.
#
# run() computes a payload in R and round-trips it through the ws-server storage API entirely in R (httr2),
# tunnelled via the /websockify SOCKS5 relay. The agent WebSocket -- used only to obtain the storage bucket's
# agent_id -- is driven through the JS-side WsClient via webr::eval_js, since webR cannot open it directly.
# httr2 is installed at runtime from the webR package repo; base R (stats/datasets) provides the computation.

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
sleep_ms <- function(ms) {
  js(sprintf("new Promise(function(resolve){setTimeout(resolve, %d);})", as.integer(ms)))
}

# Point webR's Emscripten sockets at the /websockify relay so httr2/curl reach the server through it. This runs
# in the webR worker (no await = TRUE), where the SOCKFS filesystem lives.
rdata1_configure_socket <- function(relay_url) {
  code <- paste0(
    "SOCKFS.websocketArgs = SOCKFS.websocketArgs || {};",
    " SOCKFS.websocketArgs.url = ", encodeString(relay_url, quote = "\""), ";",
    " SOCKFS.websocketArgs.subprotocol = 'binary'; 0"
  )
  webr::eval_js(code)
}

# Storage object name the payload is written to (application data, so R owns it).
rdata1_filename <- function() {
  "rdata1.txt"
}

# Compute the payload: a linear regression of fuel efficiency on weight + horsepower over the built-in
# `mtcars` dataset, summarised as deterministic CSV so the round-trip comparison is byte-stable.
rdata1_payload <- function() {
  model <- lm(mpg ~ wt + hp, data = mtcars)
  fit <- summary(model)
  rows <- data.frame(
    metric = c("observations", "mean_mpg", "sd_mpg", "intercept", "beta_wt", "beta_hp", "r_squared"),
    value = round(
      c(
        nrow(mtcars),
        mean(mtcars$mpg),
        sd(mtcars$mpg),
        coef(model)[["(Intercept)"]],
        coef(model)[["wt"]],
        coef(model)[["hp"]],
        fit$r.squared
      ),
      6
    )
  )
  paste(capture.output(write.csv(rows, row.names = FALSE)), collapse = "\n")
}

# Verify the round-trip. Returns a status line prefixed OK/FAIL so run() can decide success.
rdata1_verify <- function(sent, received) {
  if (identical(sent, received)) {
    "OK rdata1: VERIFICATION SUCCESS -- storage round-trip preserved the R payload"
  } else {
    paste0("FAIL rdata1: VERIFICATION FAILURE -- data mismatch\nSent:\n", sent, "\nGot:\n", received)
  }
}

# The control loop the JS shim hands control to.
run <- function() {
  webr::install("httr2")
  # Route curl through the /websockify relay's SOCKS5 front end (see the relay service). The proxy host is
  # nominal -- SOCKFS sends every socket to relay_url -- but the scheme must be socks5h so curl speaks SOCKS5.
  Sys.setenv(ALL_PROXY = "socks5h://127.0.0.1:8080")
  agent_log("rdata1: entered run()")

  # Point webR's sockets at this origin's /websockify relay (http(s) origin -> ws(s) relay URL).
  origin <- as.character(js("location.protocol + '//' + location.host"))
  relay_url <- sub("^http", "ws", paste0(origin, "/websockify"))
  rdata1_configure_socket(relay_url)

  # Wait for the agent WebSocket to connect and hand us the agent_id (the storage bucket).
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
  agent_log(sprintf("rdata1: registered as %s", bucket))

  # Compute in R, then PUT and GET the payload with httr2 (tunnelled to the server via the relay), and verify.
  content <- rdata1_payload()
  url <- sprintf("http://127.0.0.1:8080/storage/%s/%s", bucket, rdata1_filename())
  httr2::request(url) |>
    httr2::req_method("PUT") |>
    httr2::req_body_raw(content) |>
    httr2::req_perform()
  retrieved <- httr2::request(url) |>
    httr2::req_perform() |>
    httr2::resp_body_string()

  status <- rdata1_verify(content, retrieved)
  agent_disconnect()
  agent_log(sub("^(OK|FAIL) ", "", status))
  if (startsWith(status, "FAIL")) {
    stop("rdata1: verification failed")
  }
  agent_log("rdata1: workflow complete")
}
