//! The `no_std` ws-server: the hub's `/ws`, with `/health`, `/favicon.ico` and the modules beside it, served through
//! picoserve.
//!
//! As with [`Hub::serve`], the embedder owns the network stack and the executor, accepts each connection, and runs
//! [`serve`] for it with the one [`Hub`] every connection shares. The modules are those built into the binary, as
//! [`Modules`] describes. The other services the `std` server mounts besides the hub -- storage, websockify -- need a
//! filesystem or an OS, and are not served here.

use core::net::IpAddr;

use et_ws_service::hub::Stamps;
use et_ws_service::picoserve_hub::Hub;
use picoserve::io::Socket;
use picoserve::response::{Json, StatusCode};
use picoserve::routing::get;
use picoserve::{Config, Router, Server, Timer};

use crate::picoserve_modules::{ModuleRoutes, Modules};
use crate::routes::HealthResponse;

/// Serve one accepted connection from `client_ip` until it closes.
///
/// `GET /health` answers what the `std` server's does, `GET /favicon.ico` answers 204, and `GET /ws` is `hub`'s,
/// as [`Hub::routes`] describes. Every other request is one of `modules`', or answered 404. `config` is usually
/// [`et_ws_service::picoserve_hub::config`], and `http_buffer` holds the request head.
///
/// As for [`Hub::serve`], the crate awaiting this one needs `#![recursion_limit = "256"]`.
#[expect(
    clippy::future_not_send,
    reason = "picoserve's futures hold Cells; its executors run each connection on one thread"
)]
pub async fn serve<T, R, M, S>(
    hub: &Hub<T>,
    modules: &Modules,
    socket: S,
    client_ip: IpAddr,
    timer: M,
    config: &Config,
    http_buffer: &mut [u8],
) -> Result<(), picoserve::Error<S::Error>>
where
    T: Stamps,
    M: Timer<R>,
    S: Socket<R>,
{
    let app = hub.routes(
        Router::from_service(ModuleRoutes::new(modules))
            .route("/favicon.ico", get(async || StatusCode::NO_CONTENT))
            .route("/health", get(async || Json(HealthResponse::healthy()))),
        client_ip,
    );
    let _disconnection = Server::custom(&app, timer, config, http_buffer).serve(socket).await?;
    Ok(())
}
