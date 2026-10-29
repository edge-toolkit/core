//! The firmware: bring up the CYW43439's Wi-Fi, join the network named at build time, take an address over DHCP, and
//! serve the hub on as many sockets at once as there are connection tasks.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    clippy::unwrap_used,
    reason = "embassy's thread-mode executor runs every task on the one core; each is spawned once, or bring-up panics"
)]

use alloc::format;
use alloc::string::String;
use core::net::{IpAddr, Ipv4Addr};

use chrono::DateTime;
use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::{Debug2Format, info, warn};
use edge_toolkit::ports::Services;
use edge_toolkit::ws_server::MessageRetention;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config as NetConfig, DhcpConfig, Stack, StackResources};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, TRNG};
use embassy_rp::pio::{self, Pio};
use embassy_rp::trng::{self, Trng};
use embassy_rp::{bind_interrupts, dma};
use embassy_time::Instant;
use embedded_alloc::LlffHeap as Heap;
use et_ws_server::picoserve_server::serve;
use et_ws_service::hub::Stamps;
use et_ws_service::picoserve_hub::{self, Hub};
use picoserve::time::EmbassyTimer;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

/// The port the hub serves on, as the `std` server's plain-HTTP listener does.
const PORT: u16 = Services::InsecureWebSocketServer.port();
/// How many connections the hub serves at once: one task, and one socket's buffers, each.
const CONNECTIONS: usize = 4;
/// The largest WebSocket frame the hub reads; each connection holds a buffer this size on the heap.
const MAX_FRAME_SIZE: usize = 8 * 1024;
/// How many messages for one connection the hub holds while it writes earlier ones, each up to `MAX_FRAME_SIZE`.
const MAX_QUEUED: usize = 2;
/// Each connection's TCP receive and transmit buffer.
const TCP_BUFFER: usize = 4 * 1024;
/// Each connection's buffer for the HTTP request head.
const HTTP_BUFFER: usize = 2048;
/// The heap the hub's registry, routing and frame buffers allocate from.
const HEAP_SIZE: usize = 160 * 1024;

include!(concat!(env!("OUT_DIR"), "/board.rs"));

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    TRNG_IRQ => trng::InterruptHandler<TRNG>;
});

/// Ids unique across boots -- a word the TRNG draws at boot, then a count -- and the time since boot.
///
/// The board has no clock, so a time is its uptime counted from the Unix epoch.
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

type Radio = cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>>;

#[embassy_executor::task]
async fn radio_task(runner: Radio) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
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
        if let Err(error) = serve(hub, socket, client_ip, EmbassyTimer, &config, &mut http_buffer).await {
            warn!(
                "Connection from {} ended: {}",
                Debug2Format(&client_ip),
                Debug2Format(&error)
            );
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    #[expect(
        unsafe_code,
        reason = "handing the allocator its memory is unsafe by nature: nothing else may own that memory"
    )]
    // SAFETY: this runs once, first, before anything allocates, and `init!` declares the memory it hands over.
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }
    let peripherals = embassy_rp::init(embassy_rp::config::Config::default());
    let mut trng = Trng::new(peripherals.TRNG, Irqs, trng::Config::default());

    let power = Output::new(peripherals.PIN_23, Level::Low);
    let chip_select = Output::new(peripherals.PIN_25, Level::High);
    let mut pio = Pio::new(peripherals.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        chip_select,
        peripherals.PIN_24,
        peripherals.PIN_29,
        dma::Channel::new(peripherals.DMA_CH0, Irqs),
    );
    static RADIO: StaticCell<cyw43::State> = StaticCell::new();
    let radio_state = RADIO.init(cyw43::State::new());
    let (device, mut control, radio) = cyw43::new(radio_state, power, spi, RADIO_FIRMWARE, RADIO_NVRAM).await;
    spawner.spawn(radio_task(radio).unwrap());
    control.init(RADIO_CLM).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    static NETWORK: StaticCell<StackResources<{ CONNECTIONS + 1 }>> = StaticCell::new();
    let (stack, network) = embassy_net::new(
        device,
        NetConfig::dhcpv4(DhcpConfig::default()),
        NETWORK.init(StackResources::new()),
        trng.blocking_next_u64(),
    );
    spawner.spawn(net_task(network).unwrap());

    while let Err(error) = control
        .join(WIFI_NETWORK, JoinOptions::new(WIFI_PASSWORD.as_bytes()))
        .await
    {
        warn!("Failed to join {}: {}; retrying", WIFI_NETWORK, Debug2Format(&error));
    }
    stack.wait_config_up().await;
    if let Some(config) = stack.config_v4() {
        info!("Serving ws://{}:{}/ws", Debug2Format(&config.address.address()), PORT);
    }

    let stamps = BoardStamps {
        boot: trng.blocking_next_u32(),
        issued: 0,
    };
    static HUB: StaticCell<Hub<BoardStamps>> = StaticCell::new();
    let hub = Hub::new(stamps, MessageRetention::LatestPerSender, MAX_FRAME_SIZE, MAX_QUEUED);
    let hub: &'static Hub<BoardStamps> = HUB.init(hub);
    for _ in 0..CONNECTIONS {
        spawner.spawn(connection_task(stack, hub).unwrap());
    }
}
