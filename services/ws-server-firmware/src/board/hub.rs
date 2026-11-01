//! What every board runs once it is on the network: the hub, served on as many sockets at once as there are connection
//! tasks, from the network stack a board hands to [`serve`].

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "embassy's executor runs every task on the one core; each board calls each step once"
)]

use alloc::format;
use alloc::string::String;
use core::net::{IpAddr, Ipv4Addr};

use chrono::DateTime;
use defmt::{Debug2Format, info, warn};
use edge_toolkit::ws_server::MessageRetention;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Stack, StackResources};
use embassy_time::Instant;
use et_ws_server::picoserve_server::serve as serve_connection;
use et_ws_service::hub::Stamps;
use et_ws_service::picoserve_hub::{self, Hub};
use picoserve::time::EmbassyTimer;
use static_cell::StaticCell;

use crate::{HTTP_BUFFER, MAX_FRAME_SIZE, MAX_QUEUED, MODULES, PORT};

/// How many connections the hub serves at once: one task, and one socket's buffers, each.
const CONNECTIONS: usize = 4;
/// Each connection's TCP receive and transmit buffer.
const TCP_BUFFER: usize = 4 * 1024;
/// The heap the hub's registry, routing and frame buffers allocate from.
pub(super) const HEAP_SIZE: usize = 160 * 1024;

/// Ids unique across boots -- a random word drawn at boot, then a count -- and the time since boot.
///
/// A board has no clock, so a time is its uptime counted from the Unix epoch.
struct BoardStamps {
    boot: u32,
    issued: u32,
}

impl Stamps for BoardStamps {
    fn new_id(&mut self) -> String {
        self.issued = self.issued.wrapping_add(1);
        format!("{:08x}-{}", self.boot, self.issued)
    }

    fn now(&mut self) -> String {
        let millis = i64::try_from(Instant::now().as_millis()).unwrap_or(i64::MAX);
        DateTime::from_timestamp_millis(millis).unwrap_or_default().to_rfc3339()
    }
}

/// The network stack's resources: a socket for each connection, and one for DHCP.
pub(super) fn stack_resources() -> &'static mut StackResources<{ CONNECTIONS + 1 }> {
    static RESOURCES: StaticCell<StackResources<{ CONNECTIONS + 1 }>> = StaticCell::new();
    RESOURCES.init(StackResources::new())
}

/// Accept connections on [`PORT`] one at a time, serving each until it closes.
#[embassy_executor::task(pool_size = CONNECTIONS)]
async fn connection_task(stack: Stack<'static>, hub: &'static Hub<BoardStamps>) -> ! {
    let mut rx_buffer = [0; TCP_BUFFER];
    let mut tx_buffer = [0; TCP_BUFFER];
    let mut http_buffer = [0; HTTP_BUFFER];
    let config = picoserve_hub::config();
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        if let Err(error) = socket.accept(PORT).await {
            warn!("Failed to accept a connection: {}", error);
            continue;
        }
        let client_ip = socket
            .remote_endpoint()
            .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |remote| remote.addr.into());
        let served = serve_connection(
            hub,
            &MODULES,
            socket,
            client_ip,
            EmbassyTimer,
            &config,
            &mut http_buffer,
        );
        if let Err(error) = served.await {
            warn!(
                "Connection from {} ended: {}",
                Debug2Format(&client_ip),
                Debug2Format(&error)
            );
        }
    }
}

/// Once `stack` has an address, serve the hub on it, its ids drawn from `boot`, a random word.
pub(super) async fn serve(spawner: Spawner, stack: Stack<'static>, boot: u32) {
    static HUB: StaticCell<Hub<BoardStamps>> = StaticCell::new();
    stack.wait_config_up().await;
    if let Some(config) = stack.config_v4() {
        info!("Serving ws://{}:{}/ws", Debug2Format(&config.address.address()), PORT);
    }
    let stamps = BoardStamps { boot, issued: 0 };
    let hub = Hub::new(stamps, MessageRetention::LatestPerSender, MAX_FRAME_SIZE, MAX_QUEUED);
    let hub: &'static Hub<BoardStamps> = HUB.init(hub);
    for _ in 0..CONNECTIONS {
        match connection_task(stack, hub) {
            Ok(task) => spawner.spawn(task),
            Err(error) => warn!("Failed to start a connection task: {}", Debug2Format(&error)),
        }
    }
}
