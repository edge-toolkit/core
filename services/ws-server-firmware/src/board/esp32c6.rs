//! The ESP32-C6: bring up its Wi-Fi, join the network named at build time, rejoin it whenever it is lost, and serve the
//! hub on it.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    clippy::unwrap_used,
    reason = "esp-rtos runs embassy's executor on the one core; each task is spawned once, or bring-up panics"
)]

use core::future;

use defmt::{Debug2Format, warn};
use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, DhcpConfig, Runner};
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController};
use {esp_backtrace as _, esp_println as _};

use super::platform::HEAP_SIZE;
use super::role::{serve, stack_resources};

include!(concat!(env!("OUT_DIR"), "/wifi.rs"));

/// How long to wait before joining again, after failing to join or losing the network.
const REJOIN_DELAY: Duration = Duration::from_secs(5);
/// The heap the radio's driver allocates from, in the RAM the bootloader is done with once the firmware runs.
const RADIO_HEAP_SIZE: usize = 64 * 1024;

esp_bootloader_esp_idf::esp_app_desc!();

/// Join the network, and join it again each time it is lost or a join fails.
#[embassy_executor::task]
async fn wifi_task(mut controller: WifiController<'static>) -> ! {
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                let lost = controller.wait_for_disconnect_async().await;
                warn!("Lost {}: {}; rejoining", WIFI_NETWORK, Debug2Format(&lost));
            }
            Err(error) => warn!("Failed to join {}: {}; retrying", WIFI_NETWORK, Debug2Format(&error)),
        }
        Timer::after(REJOIN_DELAY).await;
    }
}

/// Drive the network stack over the station interface: DHCP, TCP, and every socket the hub serves on.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

/// Bring the board up, start joining the network, and serve the hub.
#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: RADIO_HEAP_SIZE);
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    let timers = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timers.timer0, peripherals.FROM_CPU_INTR0);

    let station = StationConfig::default()
        .with_ssid(WIFI_NETWORK.try_into().unwrap())
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            WIFI_PASSWORD.try_into().unwrap(),
        ));
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    )
    .unwrap();

    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32_u32) | u64::from(rng.random());
    let (stack, network) = embassy_net::new(
        Interface::station(),
        NetConfig::dhcpv4(DhcpConfig::default()),
        stack_resources(),
        seed,
    );
    spawner.spawn(wifi_task(controller).unwrap());
    spawner.spawn(net_task(network).unwrap());
    serve(spawner, stack, rng.random()).await;
    future::pending().await
}
