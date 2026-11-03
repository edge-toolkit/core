//! The hub as the firmware serves it: its `/ws`, with `/health`, `/favicon.ico`, agent storage and the modules built
//! in beside it, through picoserve.
//!
//! As with [`Hub::serve`], the board or the host owns the network stack and the executor, accepts each connection, and
//! runs [`serve`] for it with the one [`Hub`] every connection shares.

use core::net::IpAddr;

use et_modules_service::picoserve::{ModuleRoutes, Modules};
use et_storage_service::picoserve::{Storage, StorageRoutes, path};
use et_ws_service::health::HealthResponse;
use et_ws_service::hub::Stamps;
use et_ws_service::picoserve_hub::Hub;
use picoserve::io::Socket;
use picoserve::response::{Json, StatusCode};
use picoserve::routing::get;
use picoserve::{Config, Router, Server, Timer};

/// What every connection is served from.
///
/// That is the hub, the modules built in, where agents' storage is kept, and the server's settings.
#[derive(Clone, Copy)]
pub(crate) struct Served<'served, T, S> {
    hub: &'served Hub<T>,
    modules: &'served Modules,
    storage: &'served S,
    config: &'served Config,
}

impl<'served, T, S> Served<'served, T, S> {
    /// Serve `modules` and `storage` beside `hub`, as `config` says, usually [`et_ws_service::picoserve_hub::config`].
    #[expect(
        clippy::single_call_fn,
        reason = "the constructor, called where each connection is served, on the board or on the host"
    )]
    pub(crate) const fn new(
        hub: &'served Hub<T>,
        modules: &'served Modules,
        storage: &'served S,
        config: &'served Config,
    ) -> Self {
        Self {
            hub,
            modules,
            storage,
            config,
        }
    }
}

/// Serve one accepted connection from `client_ip` until it closes.
///
/// `GET /health` answers what the `std` server's does, `GET /favicon.ico` answers 204, and `GET /ws` is the hub's,
/// as [`Hub::routes`] describes. `/storage/{agent_id}/{filename}` is the storage the `std` server serves, kept in
/// `served.storage`. Every other request is one of the modules', or answered 404. `http_buffer` holds the request
/// head.
#[expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "picoserve's futures hold Cells, run on one thread; the board and the host each call it once"
)]
pub(crate) async fn serve<T, R, M, S, K>(
    served: Served<'_, T, K>,
    socket: S,
    client_ip: IpAddr,
    timer: M,
    http_buffer: &mut [u8],
) -> Result<(), picoserve::Error<S::Error>>
where
    T: Stamps,
    M: Timer<R>,
    S: Socket<R>,
    K: Storage,
{
    let storage = StorageRoutes::new(served.hub.registry(), served.storage, client_ip);
    let app = served.hub.routes(
        Router::from_service(ModuleRoutes::new(served.modules))
            .route_service(path(), storage)
            .route("/favicon.ico", get(async || StatusCode::NO_CONTENT))
            .route("/health", get(async || Json(HealthResponse::healthy()))),
        client_ip,
    );
    let _disconnection = Server::custom(&app, timer, served.config, http_buffer)
        .serve(socket)
        .await?;
    Ok(())
}
