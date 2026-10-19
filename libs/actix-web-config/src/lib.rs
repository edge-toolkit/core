//! actix-web [`HttpServer`] settings as a serde struct, so they can be read from environment variables.
//!
//! actix keeps these settings in a private struct and exposes them only as builder methods, so there is no actix type
//! to deserialize into. [`HttpServerConfig`] mirrors those methods one field each, and [`HttpServerConfig::apply`]
//! calls them on a server before it binds. Read it with `serde_env::from_env_with_prefix("HTTP")`, or nest it under a
//! field named `http` in a larger `serde-env` config; either way the variables are `HTTP_WORKERS`, `HTTP_KEEP_ALIVE`
//! and so on.
//!
//! Every field but `workers` is optional, and an unset one keeps actix's own default rather than a default restated
//! here. Those defaults live in two crates -- the connection limits in actix-server's worker config, the timeouts in
//! actix-web and actix-http -- and a copy of them here would drift on the next upgrade. The worker count always holds
//! a value, the machine's available parallelism unless set, so the config says how many workers the server runs. A
//! value actix itself rejects by panicking is rejected while the settings are read instead.
//!
//! Values are strings, as environment variables are: counts and sizes are plain integers, `shutdown_timeout` is whole
//! seconds because that is all actix takes, and the other timeouts are humantime durations such as `500ms` or `1m30s`.
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

use std::num::NonZeroUsize;
use std::time::Duration;

use actix_http::body::MessageBody;
use actix_http::{KeepAlive, Request, Response};
use actix_service::{IntoServiceFactory, Service, ServiceFactory};
use actix_web::dev::AppConfig;
use actix_web::{Error, HttpServer};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_with::{DurationSeconds, serde_as};

mod workers;

pub use self::workers::{MAX_WORKERS, Workers, WorkersError};

/// HTTP keep-alive, as actix's own [`KeepAlive`], read from a string.
///
/// `os` defers to the operating system's keep-alive, `off` or `disabled` turns keep-alive off, and anything else is a
/// humantime duration (e.g. `5s`, `1m30s`), of which `0s` also turns it off.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "String")]
pub struct KeepAliveSetting(KeepAlive);

impl TryFrom<String> for KeepAliveSetting {
    type Error = humantime::DurationError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        let value = raw.trim();
        if value.eq_ignore_ascii_case("os") {
            return Ok(Self(KeepAlive::Os));
        }
        if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("disabled") {
            return Ok(Self(KeepAlive::Disabled));
        }
        humantime::parse_duration(value).map(|duration| Self(KeepAlive::from(duration)))
    }
}

impl From<KeepAliveSetting> for KeepAlive {
    fn from(setting: KeepAliveSetting) -> Self {
        setting.0
    }
}

/// actix-web [`HttpServer`] settings, one field per builder method, each left at actix's default when unset.
#[serde_as]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct HttpServerConfig {
    /// Pending connections the listening socket queues before refusing more.
    #[serde(default)]
    pub backlog: Option<u32>,

    /// Time a client gets to close a connection before it is dropped, as a humantime duration; `0s` waits forever.
    #[serde(default, deserialize_with = "humantime_option")]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<String>"))]
    pub client_disconnect_timeout: Option<Duration>,

    /// Time a client gets to send a request's head, as a humantime duration; `0s` waits forever.
    #[serde(default, deserialize_with = "humantime_option")]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<String>"))]
    pub client_request_timeout: Option<Duration>,

    /// Whether an HTTP/1 connection stays open after the client half-closes it.
    #[serde(default)]
    pub h1_allow_half_closed: Option<bool>,

    /// Size, in bytes, an HTTP/1 response buffer reaches before it is flushed to the connection.
    #[serde(default)]
    pub h1_write_buffer_size: Option<NonZeroUsize>,

    /// Initial connection-level HTTP/2 flow-control window, in bytes.
    #[serde(default)]
    pub h2_initial_connection_window_size: Option<u32>,

    /// Initial stream-level HTTP/2 flow-control window, in bytes.
    #[serde(default)]
    pub h2_initial_window_size: Option<u32>,

    /// HTTP keep-alive: `os`, `off`/`disabled`, or a humantime duration such as `5s`.
    #[serde(default)]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<String>"))]
    pub keep_alive: Option<KeepAliveSetting>,

    /// Concurrent TLS connections each worker handles; its listeners stop accepting new ones at the limit.
    #[serde(default)]
    pub max_connection_rate: Option<NonZeroUsize>,

    /// Concurrent connections each worker serves.
    #[serde(default)]
    pub max_connections: Option<NonZeroUsize>,

    /// Host name the application router uses when it generates URLs.
    #[serde(default)]
    pub server_hostname: Option<String>,

    /// Seconds running connections get to finish once a graceful shutdown starts.
    #[serde(default)]
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<u64>"))]
    pub shutdown_timeout: Option<Duration>,

    /// Whether `TCP_NODELAY` is set on accepted connections.
    #[serde(default)]
    pub tcp_nodelay: Option<bool>,

    /// Time a client gets to complete the TLS handshake, as a humantime duration.
    #[cfg(feature = "rustls-0_23")]
    #[serde(default, deserialize_with = "humantime_option")]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<String>"))]
    pub tls_handshake_timeout: Option<Duration>,

    /// Size of each worker's pool of threads for blocking work.
    #[serde(default)]
    pub worker_max_blocking_threads: Option<NonZeroUsize>,

    /// Worker threads per bind address, from 1 to 512; unset, the machine's available parallelism.
    #[serde(default)]
    #[cfg_attr(feature = "env-schema", schemars(with = "usize"))]
    pub workers: Workers,
}

/// Read a set field as a humantime duration (e.g. `500ms`, `1m30s`).
fn humantime_option<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    humantime_serde::deserialize(deserializer).map(Some)
}

impl HttpServerConfig {
    /// Apply every setting that is set to `server`, leaving the rest at actix's defaults.
    pub fn apply<F, I, S, B>(&self, server: HttpServer<F, I, S, B>) -> HttpServer<F, I, S, B>
    where
        F: Fn() -> I + Send + Clone + 'static,
        I: IntoServiceFactory<S, Request>,
        S: ServiceFactory<Request, Config = AppConfig> + 'static,
        S::Error: Into<Error> + 'static,
        S::InitError: core::fmt::Debug,
        S::Response: Into<Response<B>> + 'static,
        <S::Service as Service<Request>>::Future: 'static,
        S::Service: 'static,
        B: MessageBody + 'static,
    {
        let srv = server.workers(self.workers.into_inner());
        let srv = set(srv, self.worker_max_blocking_threads, |srv, num| {
            srv.worker_max_blocking_threads(num.get())
        });
        let srv = set(srv, self.backlog, HttpServer::backlog);
        let srv = set(srv, self.max_connections, |srv, num| srv.max_connections(num.get()));
        let srv = set(srv, self.max_connection_rate, |srv, num| {
            srv.max_connection_rate(num.get())
        });
        let srv = set(srv, self.tcp_nodelay, HttpServer::tcp_nodelay);
        let srv = set(srv, self.h1_allow_half_closed, HttpServer::h1_allow_half_closed);
        let srv = set(srv, self.h1_write_buffer_size, |srv, num| {
            srv.h1_write_buffer_size(num.get())
        });
        let srv = set(srv, self.h2_initial_window_size, HttpServer::h2_initial_window_size);
        let srv = set(
            srv,
            self.h2_initial_connection_window_size,
            HttpServer::h2_initial_connection_window_size,
        );
        let srv = set(srv, self.server_hostname.as_deref(), HttpServer::server_hostname);
        let srv = set(srv, self.keep_alive, |srv, alive| {
            srv.keep_alive(KeepAlive::from(alive))
        });
        let srv = set(srv, self.client_request_timeout, HttpServer::client_request_timeout);
        let srv = set(
            srv,
            self.client_disconnect_timeout,
            HttpServer::client_disconnect_timeout,
        );
        #[cfg(feature = "rustls-0_23")]
        let srv = set(srv, self.tls_handshake_timeout, HttpServer::tls_handshake_timeout);
        set(srv, self.shutdown_timeout, |srv, secs| {
            srv.shutdown_timeout(secs.as_secs())
        })
    }
}

/// Pass `server` through `setter` with `value` when it is set, and return it unchanged when it is not.
fn set<T, V>(server: T, value: Option<V>, setter: impl FnOnce(T, V) -> T) -> T {
    match value {
        Some(value) => setter(server, value),
        None => server,
    }
}
