//! The firmware's hub and modules, served from the host over tokio rather than from the board over its Wi-Fi.
//!
//! The hub, its settings and the modules built in are the board's own; only the network stack and the executor
//! differ. It listens on the address its one argument names, or on the board's port on every interface, and says
//! where on its first line of output, so a caller can ask for port 0 and learn the one it got.

use std::error::Error;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use edge_toolkit::ws_server::MessageRetention;
use et_ws_server::picoserve_server::serve;
use et_ws_service::hub::Stamps;
use et_ws_service::picoserve_hub::{self, Hub};
use picoserve::time::TokioTimer;
use tokio::net::TcpListener;
use tokio::task::LocalSet;

use crate::{HTTP_BUFFER, MAX_FRAME_SIZE, MAX_QUEUED, MODULES, PORT};

/// Ids `host-1`, `host-2`, ... in turn, and the host's clock, which unlike the board's knows the date.
#[derive(Default)]
struct HostStamps(u32);

impl Stamps for HostStamps {
    fn new_id(&mut self) -> String {
        self.0 = self.0.wrapping_add(1);
        format!("host-{}", self.0)
    }

    fn now(&mut self) -> String {
        DateTime::<Utc>::from(SystemTime::now()).to_rfc3339()
    }
}

/// Serve the hub on the address the first argument names, or on [`PORT`] on every interface, until killed.
pub(crate) fn main() -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("0.0.0.0:{PORT}"))
        .parse()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    // One thread, as the board's executor has: picoserve's connections are not `Send`.
    LocalSet::new().block_on(&runtime, Box::pin(accept(address)))
}

/// Accept connections on `address`, serving each from its own task with the one hub they share.
#[expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "picoserve's futures hold Cells, run on one thread as on the board; a step apart from the runtime's setup"
)]
async fn accept(address: SocketAddr) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(address).await?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "Serving http://{}/", listener.local_addr()?)?;
    stdout.flush()?;
    let hub = Arc::new(Hub::new(
        HostStamps::default(),
        MessageRetention::LatestPerSender,
        MAX_FRAME_SIZE,
        MAX_QUEUED,
    ));
    loop {
        let (socket, peer) = listener.accept().await?;
        let hub = Arc::clone(&hub);
        let _serving = tokio::task::spawn_local(async move {
            // Each connection's own, as each of the board's connection tasks holds one.
            let config = picoserve_hub::config();
            let mut http_buffer = [0; HTTP_BUFFER];
            // A connection that ends badly is the client's to report; the hub carries on serving the others.
            let _served = serve(&hub, &MODULES, socket, peer.ip(), TokioTimer, &config, &mut http_buffer).await;
        });
    }
}
