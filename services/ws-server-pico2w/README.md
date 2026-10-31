# et-int-ws-server-pico2w

et-ws-server as firmware for the Raspberry Pi Pico 2 W, built as the `et-ws-server-pico2w` binary: the `no_std`
hub's `/ws`, with `/health`, `/favicon.ico` and the modules built into it beside it, served over the board's own
Wi-Fi on port 8080. It joins the network it was built for, takes an address over DHCP, and serves four connections
at once.

It is a local hub, with no backplane, admin API, idle timeout or registry file. The board has no clock, so the times
it stamps on messages are its uptime counted from the Unix epoch.

## Building

The build bakes in the network to join, from `WIFI_NETWORK` and `WIFI_PASSWORD`, and the CYW43439 radio's firmware.
The build task fetches the radio's `43439A0.bin`, `43439A0_clm.bin` and `nvram_rp2040.bin` from the embassy
repository's `cyw43-firmware` directory into `target/cyw43-firmware`, and points `CYW43_FIRMWARE_DIR` at them.
Infineon licenses those blobs separately from this project, so they are not part of it.

It serves no modules unless told which to build in, through the variables the `std` server reads, each optional:

- `MODULES_PATHS`: the module directories, comma-separated and absolute. Each is built in from its `pkg/` when it has
  one, otherwise from the directory itself: every file its `package.json` lists under `files`, or every file there.
- `MODULES_ROOT`: the name of the module served at `/`, one of those.
- `MODULES_ELSEWHERE` and `MODULES_ELSEWHERE_URL`: module names too big for the board's 4 MiB of flash, listed with
  the rest at `/modules/`, and the base URL a request for one of their files is redirected to.

`et-cli generate-deployment --output-type firmware-pico2` writes the task that sets all of these for a scenario.

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

## Serving from the host

Built for any other target, the binary is the same hub serving the same modules from the host over tokio, read from
the same `MODULES_*` at build time, so what a board would serve can be explored without one. It listens on the
address its one argument names, or on port 8080 on every interface, and prints where:

```sh
cargo run -p et-int-ws-server-pico2w -- 127.0.0.1:8080
```

The package's test starts it on a free port and fetches every module it lists, so run with the `MODULES_*` a
scenario's deployment sets, as its `firmware-host-test` task does, it checks what that board would serve.
