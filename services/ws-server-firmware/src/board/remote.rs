//! A board whose guests reach a hub on another machine: it serves nothing, and drives each guest's sockets to the hub
//! at [`HUB_ADDRESS`].

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "embassy's executor runs every task on the one core; each board calls each step once"
)]

use alloc::format;
use alloc::string::String;

use defmt::{Debug2Format, info, warn};
use embassy_executor::Spawner;
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::{Error, TcpReader, TcpSocket, TcpWriter};
use embassy_net::{IpEndpoint, Stack, StackResources};
use embedded_io_async::{ErrorType, Read, Write};
use et_ws_wasi_runner::board::net::{Connector, NetError, NetLink, NetTransport, Split, drive};

use super::platform::{BoardPlatform, spawn_runner};
use crate::HUB_ADDRESS;
use crate::runner::{GUEST_POOL, GUESTS, Shared, run_guest};

/// Each socket's TCP receive and transmit buffer.
const TCP_BUFFER: usize = 4 * 1024;
/// The network stack's sockets: two for each guest, its websocket's and its storage requests', and DHCP and DNS.
const SOCKETS: usize = 2 * GUEST_POOL + 2;

/// Each guest's link to the task driving its sockets.
static LINKS: [NetLink; GUEST_POOL] = [const { NetLink::idle() }; GUEST_POOL];

/// The network stack's resources.
pub(super) fn stack_resources() -> &'static mut StackResources<SOCKETS> {
    static RESOURCES: static_cell::StaticCell<StackResources<SOCKETS>> = static_cell::StaticCell::new();
    RESOURCES.init(StackResources::new())
}

/// A socket of the board's to the hub.
struct BoardSocket<'socket>(TcpSocket<'socket>);

impl ErrorType for BoardSocket<'_> {
    type Error = Error;
}

impl Read for BoardSocket<'_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.0.read(buf).await
    }
}

impl Write for BoardSocket<'_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Error> {
        self.0.write(buf).await
    }

    async fn flush(&mut self) -> Result<(), Error> {
        self.0.flush().await
    }
}

impl Split for BoardSocket<'_> {
    type Reader<'half>
        = TcpReader<'half>
    where
        Self: 'half;
    type Writer<'half>
        = TcpWriter<'half>
    where
        Self: 'half;

    fn split(&mut self) -> (TcpReader<'_>, TcpWriter<'_>) {
        self.0.split()
    }
}

/// Opens a socket to the hub on `stack`, through buffers of its own, each time it is asked.
struct BoardConnector {
    stack: Stack<'static>,
    rx_buffer: [u8; TCP_BUFFER],
    tx_buffer: [u8; TCP_BUFFER],
}

impl BoardConnector {
    /// A connector on `stack`.
    const fn new(stack: Stack<'static>) -> Self {
        Self {
            stack,
            rx_buffer: [0; TCP_BUFFER],
            tx_buffer: [0; TCP_BUFFER],
        }
    }
}

impl Connector for BoardConnector {
    type Socket<'socket> = BoardSocket<'socket>;

    /// A socket to the hub at [`HUB_ADDRESS`], its host looked up first.
    async fn connect(&mut self) -> Result<BoardSocket<'_>, NetError> {
        let endpoint = hub_endpoint(self.stack).await?;
        let mut socket = TcpSocket::new(self.stack, &mut self.rx_buffer, &mut self.tx_buffer);
        match socket.connect(endpoint).await {
            Ok(()) => Ok(BoardSocket(socket)),
            Err(error) => Err(NetError::Connect(format!("{error:?}"))),
        }
    }
}

/// Where [`HUB_ADDRESS`] is, its host looked up on `stack` unless it is an address already.
async fn hub_endpoint(stack: Stack<'static>) -> Result<IpEndpoint, NetError> {
    let unreachable = |reason: String| Err(NetError::Connect(reason));
    let Some((host, port)) = HUB_ADDRESS.rsplit_once(':') else {
        return unreachable(format!("{HUB_ADDRESS} names no port"));
    };
    let Ok(port) = port.parse() else {
        return unreachable(format!("{HUB_ADDRESS} names no port"));
    };
    match stack.dns_query(host, DnsQueryType::A).await {
        Ok(addresses) => addresses.first().map_or_else(
            || unreachable(format!("{host} resolves to no address")),
            |address| Ok(IpEndpoint::new(*address, port)),
        ),
        Err(error) => unreachable(format!("{error:?}")),
    }
}

/// Drive one guest's sockets to the hub, for as long as the board runs.
#[embassy_executor::task(pool_size = GUEST_POOL)]
async fn net_task(stack: Stack<'static>, link: &'static NetLink) -> ! {
    #[expect(
        clippy::large_futures,
        reason = "the sockets' buffers are meant to be in it: a task's future is the static memory embassy keeps it in"
    )]
    drive(
        link,
        BoardConnector::new(stack),
        BoardConnector::new(stack),
        HUB_ADDRESS,
        BoardPlatform,
    )
    .await
}

/// Run one guest the build compiled in against the hub, until it ends.
#[embassy_executor::task(pool_size = GUEST_POOL)]
async fn guest_task(link: &'static NetLink, name: &'static str, component: &'static [u8], shared: Shared) {
    let transport = || NetTransport::new(link, BoardPlatform);
    run_guest(name, component, BoardPlatform, transport, shared.memory()).await;
}

/// Once `stack` has an address, run each guest the build compiled in against the hub at [`HUB_ADDRESS`].
///
/// `_boot`, the random word a hub draws its ids from, is not needed: the hub elsewhere assigns them.
pub(super) async fn serve(spawner: Spawner, stack: Stack<'static>, _boot: u32) {
    stack.wait_config_up().await;
    info!("Joining ws://{=str}/ws", HUB_ADDRESS);
    let shared = Shared::new();
    for ((name, component), link) in GUESTS.iter().zip(&LINKS) {
        match net_task(stack, link) {
            Ok(task) => spawner.spawn(task),
            Err(error) => warn!(
                "Failed to start runner {=str}'s sockets: {}",
                name,
                Debug2Format(&error)
            ),
        }
        spawn_runner(spawner, guest_task(link, name, component, shared.clone()), name);
    }
}
