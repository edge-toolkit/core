//! How `HTTP_*` environment variables become actix `HttpServer` settings, and that they reach the running server.
#![cfg(test)]

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use actix_http::KeepAlive;
use actix_web::{App, HttpServer, web};
use et_actix_web_config::{HttpServerConfig, KeepAliveSetting, MAX_WORKERS, Workers, WorkersError};

/// Every setting, each at a value other than actix's default.
///
/// Keep-alive is `off` so a served response shows the settings reached the server: with keep-alive off it says it will
/// close the connection, which by default it does not.
const EVERY_SETTING: [(&str, &str); 15] = [
    ("HTTP_BACKLOG", "64"),
    ("HTTP_CLIENT_DISCONNECT_TIMEOUT", "500ms"),
    ("HTTP_CLIENT_REQUEST_TIMEOUT", "2s"),
    ("HTTP_H1_ALLOW_HALF_CLOSED", "false"),
    ("HTTP_H1_WRITE_BUFFER_SIZE", "16000"),
    ("HTTP_H2_INITIAL_CONNECTION_WINDOW_SIZE", "4000000"),
    ("HTTP_H2_INITIAL_WINDOW_SIZE", "2000000"),
    ("HTTP_KEEP_ALIVE", "off"),
    ("HTTP_MAX_CONNECTION_RATE", "10"),
    ("HTTP_MAX_CONNECTIONS", "100"),
    ("HTTP_SERVER_HOSTNAME", "edge.example"),
    ("HTTP_SHUTDOWN_TIMEOUT", "10"),
    ("HTTP_TCP_NODELAY", "true"),
    ("HTTP_WORKER_MAX_BLOCKING_THREADS", "4"),
    ("HTTP_WORKERS", "2"),
];

fn read(vars: &[(&str, &str)]) -> Result<HttpServerConfig, serde_env::Error> {
    let mut all = vars.to_vec();
    if cfg!(feature = "rustls-0_23") {
        all.push(("HTTP_TLS_HANDSHAKE_TIMEOUT", "7s"));
    }
    serde_env::from_iter_with_prefix(all, "HTTP")
}

#[test]
fn every_count_and_size_is_read_from_its_http_variable() {
    let config = read(&EVERY_SETTING).unwrap();

    assert_eq!(config.backlog, Some(64));
    assert_eq!(config.h1_write_buffer_size.map(usize::from), Some(16_000));
    assert_eq!(config.h2_initial_connection_window_size, Some(4_000_000));
    assert_eq!(config.h2_initial_window_size, Some(2_000_000));
    assert_eq!(config.max_connection_rate.map(usize::from), Some(10));
    assert_eq!(config.max_connections.map(usize::from), Some(100));
    assert_eq!(config.worker_max_blocking_threads.map(usize::from), Some(4));
    assert_eq!(config.workers.into_inner(), 2);
}

#[test]
fn every_timeout_and_flag_is_read_from_its_http_variable() {
    let config = read(&EVERY_SETTING).unwrap();

    assert_eq!(config.client_disconnect_timeout, Some(Duration::from_millis(500)));
    assert_eq!(config.client_request_timeout, Some(Duration::from_secs(2)));
    assert_eq!(config.h1_allow_half_closed, Some(false));
    assert_eq!(config.keep_alive.map(KeepAlive::from), Some(KeepAlive::Disabled));
    assert_eq!(config.server_hostname.as_deref(), Some("edge.example"));
    assert_eq!(config.shutdown_timeout, Some(Duration::from_secs(10)));
    assert_eq!(config.tcp_nodelay, Some(true));
    #[cfg(feature = "rustls-0_23")]
    assert_eq!(config.tls_handshake_timeout, Some(Duration::from_secs(7)));
}

#[test]
fn an_unset_setting_is_left_to_actix() {
    let config = HttpServerConfig::default();

    assert!(config.keep_alive.is_none());
    assert!(config.client_request_timeout.is_none());
    assert!(config.shutdown_timeout.is_none());
}

#[test]
fn a_value_actix_would_panic_on_is_rejected_while_reading() {
    // actix-server panics on a worker count outside 1..=512, and the other counts are meaningless at zero.
    let accepted: Vec<(&str, &str)> = [
        ("HTTP_WORKERS", "0"),
        ("HTTP_WORKERS", "513"),
        ("HTTP_MAX_CONNECTIONS", "0"),
        ("HTTP_WORKER_MAX_BLOCKING_THREADS", "0"),
        ("HTTP_H1_WRITE_BUFFER_SIZE", "0"),
        ("HTTP_KEEP_ALIVE", "soon"),
        ("HTTP_CLIENT_REQUEST_TIMEOUT", "soon"),
        ("HTTP_SHUTDOWN_TIMEOUT", "10s"),
    ]
    .into_iter()
    .filter(|var| read(&[*var]).is_ok())
    .collect();

    assert_eq!(accepted, []);
}

#[test]
fn the_worker_count_is_bounded_by_what_actix_accepts() {
    assert_eq!(Workers::try_new(MAX_WORKERS).map(Workers::into_inner), Ok(512));
    assert_eq!(Workers::try_new(0), Err(WorkersError::GreaterOrEqualViolated));
    assert_eq!(Workers::try_new(513), Err(WorkersError::LessOrEqualViolated));
}

#[test]
fn an_unset_worker_count_is_the_machines_available_parallelism() {
    let parallelism = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);

    assert_eq!(
        HttpServerConfig::default().workers.into_inner(),
        parallelism.min(MAX_WORKERS)
    );
    let config: HttpServerConfig = serde_env::from_iter_with_prefix([("HTTP_BACKLOG", "8")], "HTTP").unwrap();
    assert_eq!(config.workers, Workers::default());
}

#[test]
fn actix_accepts_the_most_workers_the_config_allows() {
    let _server = HttpServer::new(App::new).workers(MAX_WORKERS);
}

#[test]
#[should_panic(expected = "Workers must not exceed 512")]
fn actix_refuses_one_worker_more_than_the_config_allows() {
    let _server = HttpServer::new(App::new).workers(MAX_WORKERS + 1);
}

#[test]
fn keep_alive_takes_os_a_disable_word_or_a_duration() {
    let parse = |raw: &str| KeepAlive::from(KeepAliveSetting::try_from(raw.to_owned()).unwrap());

    assert_eq!(parse("os"), KeepAlive::Os);
    assert_eq!(parse(" OS "), KeepAlive::Os);
    assert_eq!(parse("off"), KeepAlive::Disabled);
    assert_eq!(parse("Disabled"), KeepAlive::Disabled);
    assert_eq!(parse("0s"), KeepAlive::Disabled);
    assert_eq!(parse("1m30s"), KeepAlive::Timeout(Duration::from_secs(90)));
}

/// Start `config` on an ephemeral port, send one HTTP/1.1 request, and return the raw response, lower-cased.
fn served_response(config: &HttpServerConfig) -> String {
    let server = HttpServer::new(|| App::new().route("/", web::get().to(|| async { "ok" })));
    let server = config.apply(server).bind(("127.0.0.1", 0)).unwrap();
    let address = server.addrs()[0];
    let running = server.run();
    let handle = running.handle();
    actix_rt::System::new().block_on(async move {
        let _server = actix_rt::spawn(running);
        let response = actix_rt::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
            let mut buffer = [0_u8; 1024];
            let length = stream.read(&mut buffer).unwrap();
            String::from_utf8_lossy(&buffer[..length]).to_lowercase()
        })
        .await
        .unwrap();
        handle.stop(false).await;
        response
    })
}

#[test]
fn every_applied_setting_reaches_a_server_that_still_serves() {
    let response = served_response(&read(&EVERY_SETTING).unwrap());

    assert!(response.starts_with("http/1.1 200"));
    assert!(response.contains("connection: close"));
}

#[test]
fn a_server_left_at_the_defaults_keeps_the_connection_alive() {
    let response = served_response(&HttpServerConfig::default());

    assert!(response.starts_with("http/1.1 200"));
    assert!(!response.contains("connection: close"));
}
