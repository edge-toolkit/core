//! The Raspberry Pi Pico 2 W: bring up the CYW43439's Wi-Fi, join the network named at build time, and serve the hub
//! on it.
//!
//! With the `pico-plus2w` feature it is the Pimoroni Pico Plus 2 W instead, the same radio wired the same way to an
//! RP2350B, whose PSRAM it maps in and makes the heap before anything allocates.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    clippy::unwrap_used,
    reason = "embassy's thread-mode executor runs every task on the one core; each is spawned once, or bring-up panics"
)]

use cyw43::JoinOptions;
use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::{Debug2Format, warn};
use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, DhcpConfig};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, TRNG};
use embassy_rp::pio::{self, Pio};
use embassy_rp::trng::{self, Trng};
use embassy_rp::{bind_interrupts, dma};
use embedded_alloc::LlffHeap as Heap;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

#[cfg(not(feature = "pico-plus2w"))]
use super::platform::HEAP_SIZE;
use super::role::{serve, stack_resources};

include!(concat!(env!("OUT_DIR"), "/wifi.rs"));
include!(concat!(env!("OUT_DIR"), "/cyw43.rs"));

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    TRNG_IRQ => trng::InterruptHandler<TRNG>;
});

type Radio = cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>>;

/// Drive the CYW43439, which every frame the board sends or receives passes through.
#[embassy_executor::task]
async fn radio_task(runner: Radio) -> ! {
    runner.run().await
}

/// Drive the network stack over the radio: DHCP, TCP, and every socket the hub serves on.
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

/// Bring the board up, join the network, and serve the hub.
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    #[expect(
        unsafe_code,
        reason = "handing the allocator its memory is unsafe by nature: nothing else may own that memory"
    )]
    // SAFETY: this runs once, first, before anything allocates, and `init!` declares the memory it hands over.
    #[cfg(not(feature = "pico-plus2w"))]
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }
    let peripherals = embassy_rp::init(embassy_rp::config::Config::default());
    // The Pico Plus 2 W's heap is its 8 MiB of PSRAM instead, which its guests' memories need, mapped in first.
    #[cfg(feature = "pico-plus2w")]
    {
        use embassy_rp::psram::{Config as PsramConfig, Psram};
        use embassy_rp::qmi_cs1::QmiCs1;

        let config = PsramConfig {
            clock_hz: embassy_rp::clocks::clk_sys_freq(),
            ..PsramConfig::aps6404l()
        };
        let psram = Psram::new(QmiCs1::new(peripherals.QMI_CS1, peripherals.PIN_47), config).unwrap();
        #[expect(
            unsafe_code,
            reason = "handing the allocator its memory is unsafe by nature: nothing else may own that memory"
        )]
        // SAFETY: nothing has allocated yet, and the PSRAM window is mapped and belongs to nothing else.
        unsafe {
            HEAP.init(psram.base_address().addr(), psram.size());
        }
    }
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

    let (stack, network) = embassy_net::new(
        device,
        NetConfig::dhcpv4(DhcpConfig::default()),
        stack_resources(),
        trng.blocking_next_u64(),
    );
    spawner.spawn(net_task(network).unwrap());

    while let Err(error) = control
        .join(WIFI_NETWORK, JoinOptions::new(WIFI_PASSWORD.as_bytes()))
        .await
    {
        warn!("Failed to join {}: {}; retrying", WIFI_NETWORK, Debug2Format(&error));
    }
    serve(spawner, stack, trng.blocking_next_u32()).await;
}
