//! The firmware's hub, modules and runners, served from the host over tokio rather than from the board over its Wi-Fi.
//!
//! The hub, its settings, the modules built in and the guests its runners run are the board's own; only the network
//! stack, the executor and the clock differ. It listens on the address its one argument names, or on the board's port
//! on every interface, and says where on its first line of output, so a caller can ask for port 0 and learn the one
//! it got. A build for a hub elsewhere serves nothing, and runs its guests against that hub as the board would.

use std::error::Error;
use std::io::Write as _;
#[cfg(not(remote_hub))]
use std::net::SocketAddr;
use std::sync::LazyLock;
#[cfg(not(remote_hub))]
use std::time::SystemTime;
use std::time::{Duration, Instant};

#[cfg(not(remote_hub))]
use chrono::{DateTime, Utc};
// The host's critical section, which the guests' links lock under, as a board's HAL provides on the board.
use critical_section as _;
#[cfg(not(remote_hub))]
use edge_toolkit::ws_server::MessageRetention;
#[cfg(not(remote_hub))]
use et_ws_service::hub::Stamps;
#[cfg(not(remote_hub))]
use et_ws_service::picoserve_hub::{self, Hub};
use et_ws_wasi_runner::bindings::wasi::logging::logging::Level;
#[cfg(remote_hub)]
use et_ws_wasi_runner::board::net::{NetLink, NetTransport, drive};
#[cfg(remote_hub)]
use et_ws_wasi_runner::board::net_tokio::TokioConnector;
use et_ws_wasi_runner::board::{Console, Platform};
#[cfg(not(remote_hub))]
use picoserve::time::TokioTimer;
#[cfg(not(remote_hub))]
use tokio::net::TcpListener;
use tokio::task::LocalSet;

#[cfg(remote_hub)]
use crate::HUB_ADDRESS;
#[cfg(not(remote_hub))]
use crate::local::BoardStorage;
use crate::runner::{GUESTS, Shared, run_guest};
#[cfg(not(remote_hub))]
use crate::server::{Served, serve};
#[cfg(not(remote_hub))]
use crate::{HTTP_BUFFER, MAX_FRAME_SIZE, MAX_QUEUED, MODULES, PORT};

/// When the process started, which the guests' clock counts from.
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The host's monotonic clock and its standard streams, standing in for a board's clock and log.
#[derive(Clone, Copy)]
struct HostPlatform;

impl Platform for HostPlatform {
    /// The nanoseconds since the process started, as a board counts its uptime.
    fn now(&self) -> u64 {
        u64::try_from(STARTED.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Wait on tokio's clock until [`HostPlatform::now`] reaches `deadline`.
    async fn sleep_until(&self, deadline: u64) {
        match STARTED.checked_add(Duration::from_nanos(deadline)) {
            Some(due) => tokio::time::sleep_until(due.into()).await,
            // A deadline past what the clock can count never comes.
            None => std::future::pending().await,
        }
    }

    /// Write the guest's log entry to standard error, its level named.
    fn log(&self, level: Level, context: &str, message: &str) {
        let level = match level {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Critical => "CRITICAL",
        };
        let mut stderr = std::io::stderr().lock();
        let _written = writeln!(stderr, "{level} {context}: {message}");
    }

    /// Write what the guest wrote to the host's own stream of the same name.
    fn write(&self, console: Console, bytes: &[u8]) {
        let _written = if matches!(console, Console::Stdout) {
            std::io::stdout().lock().write_all(bytes)
        } else {
            std::io::stderr().lock().write_all(bytes)
        };
    }
}

/// Ids `host-1`, `host-2`, ... in turn, and the host's clock, which unlike the board's knows the date.
#[cfg(not(remote_hub))]
#[derive(Default)]
struct HostStamps(u32);

#[cfg(not(remote_hub))]
impl Stamps for HostStamps {
    fn new_id(&mut self) -> String {
        self.0 = self.0.wrapping_add(1);
        format!("host-{}", self.0)
    }

    fn now(&mut self) -> String {
        DateTime::<Utc>::from(SystemTime::now()).to_rfc3339()
    }
}

/// Run what the board would until killed: serve the hub, or with a hub elsewhere, run the guests against it.
pub(crate) fn main() -> Result<(), Box<dyn Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    // One thread, as the board's executor has: picoserve's connections are not `Send`.
    LocalSet::new().block_on(&runtime, Box::pin(start()))
}

/// Run each guest the build compiled in against the hub at [`HUB_ADDRESS`], until every one has ended.
///
/// Says where on its first line of output, as the hub's own build does.
#[cfg(remote_hub)]
#[expect(clippy::single_call_fn, reason = "a step apart from the runtime's setup")]
async fn start() -> Result<(), Box<dyn Error>> {
    let address = tokio::net::lookup_host(HUB_ADDRESS)
        .await?
        .next()
        .ok_or("HUB_ADDRESS names no address")?;
    {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "Joining ws://{HUB_ADDRESS}/ws with {} runners", GUESTS.len())?;
        stdout.flush()?;
    }
    let shared = Shared::new();
    let mut running = Vec::new();
    for (name, component) in GUESTS {
        // Leaked, as the board's live in statics: the guest and the task driving its sockets hold it to the end.
        let link: &'static NetLink = Box::leak(Box::new(NetLink::idle()));
        let dial = TokioConnector::new(address);
        let _driving = tokio::task::spawn_local(async move {
            drive(link, dial, dial, HUB_ADDRESS, HostPlatform).await;
        });
        let transport = move || NetTransport::new(link, HostPlatform);
        let guest = run_guest(name, component, HostPlatform, transport, shared.memory());
        running.push(tokio::task::spawn_local(guest));
    }
    for guest in running {
        guest.await?;
    }
    Ok(())
}

/// Serve the hub on the address the first argument names, or on [`PORT`] on every interface.
///
/// Accepts connections there, serving each from its own task with the one hub they share.
#[cfg(not(remote_hub))]
#[expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "picoserve's futures hold Cells, run on one thread as on the board; a step apart from the runtime's setup"
)]
async fn start() -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("0.0.0.0:{PORT}"))
        .parse()?;
    let listener = TcpListener::bind(address).await?;
    let mut stdout = std::io::stdout().lock();
    // With how many runners it starts, so a caller knows how many agents of its own to expect.
    writeln!(
        stdout,
        "Serving http://{}/ with {} runners",
        listener.local_addr()?,
        GUESTS.len()
    )?;
    stdout.flush()?;
    // Leaked, as the board's lives in a static: it serves until the process ends, and every runner holds it that long.
    let hub: &'static Hub<HostStamps> = Box::leak(Box::new(Hub::new(
        HostStamps::default(),
        MessageRetention::LatestPerSender,
        MAX_FRAME_SIZE,
        MAX_QUEUED,
    )));
    let shared = Shared::new();
    let storage: &'static BoardStorage = Box::leak(Box::new(shared.storage()));
    for (name, component) in GUESTS {
        let guest_shared = shared.clone();
        let transport = move || guest_shared.hub_transport(hub, HostPlatform);
        let guest = run_guest(name, component, HostPlatform, transport, shared.memory());
        let _running = tokio::task::spawn_local(guest);
    }
    loop {
        let (socket, peer) = listener.accept().await?;
        let _serving = tokio::task::spawn_local(async move {
            // Each connection's own, as each of the board's connection tasks holds one.
            let config = picoserve_hub::config();
            let mut http_buffer = [0; HTTP_BUFFER];
            let served = Served::new(hub, &MODULES, storage, &config);
            // A connection that ends badly is the client's to report; the hub carries on serving the others.
            let _served = serve(served, socket, peer.ip(), TokioTimer, &mut http_buffer).await;
        });
    }
}
