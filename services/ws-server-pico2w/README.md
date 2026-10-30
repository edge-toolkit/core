# et-int-ws-server-pico2w

et-ws-server as firmware for the Raspberry Pi Pico 2 W, built as the `et-ws-server-pico2w` binary: the `no_std`
hub's `/ws`, with `/health` and `/favicon.ico` beside it, served over the board's own Wi-Fi on port 8080. It joins
the network it was built for, takes an address over DHCP, and serves four connections at once.

It is a local hub, with no backplane, admin API, idle timeout or registry file. The board has no clock, so the times
it stamps on messages are its uptime counted from the Unix epoch.

## Building

The build bakes in the network to join and the CYW43439 radio's firmware, so it needs three variables:

- `WIFI_NETWORK` and `WIFI_PASSWORD`: the network.
- `CYW43_FIRMWARE_DIR`: a directory holding the radio's `43439A0.bin`, `43439A0_clm.bin` and `nvram_rp2040.bin`, as
  the `cyw43-firmware` directory of the embassy repository does. Infineon licenses those blobs separately from this
  project, so they are not part of it.

```sh
MISE_ENV=rust mise run build-firmware-pico2w
```

The firmware is `target/thumbv8m.main-none-eabihf/release/et-ws-server-pico2w`, an ELF. Load it with picotool while
the board is in BOOTSEL mode, or with probe-rs over a debug probe, which also shows its defmt log:

```sh
picotool load -x -t elf target/thumbv8m.main-none-eabihf/release/et-ws-server-pico2w
```

```sh
probe-rs run --chip RP235x target/thumbv8m.main-none-eabihf/release/et-ws-server-pico2w
```

The log names the address it serves on, as `ws://<address>:8080/ws`.
