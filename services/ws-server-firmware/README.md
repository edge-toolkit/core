# et-int-ws-server-firmware

et-ws-server as firmware, built as the `et-ws-server-firmware` binary: the `no_std` hub's `/ws`, with `/health`,
`/favicon.ico` and the modules built into it beside it, served over a board's own Wi-Fi on port 8080. It joins the
network it was built for, takes an address over DHCP, and serves four connections at once.

The target it is built for, and for an Arm board its feature, picks the board:

| Board                  | Target                         | Feature       | Build task                   | Loaded with |
| ---------------------- | ------------------------------ | ------------- | ---------------------------- | ----------- |
| Raspberry Pi Pico 2 W  | `thumbv8m.main-none-eabihf`    | `pico2w`      | `build-firmware-pico2w`      | picotool    |
| Pimoroni Pico Plus 2 W | `thumbv8m.main-none-eabihf`    | `pico-plus2w` | `build-firmware-pico-plus2w` | picotool    |
| ESP32-C6               | `riscv32imac-unknown-none-elf` |               | `build-firmware-esp32c6`     | espflash    |

Everything but bringing up the radio is shared, so every board serves the same hub the same way. It is a local hub,
with no backplane, admin API, idle timeout or registry file. A board has no clock, so the times it stamps on messages
are its uptime counted from the Unix epoch.

The Pico Plus 2 W is the Pico 2 W's radio on an RP2350B with 8 MiB of PSRAM, which it maps in and makes the heap.
That is room for guests, so it also runs the wasi runners the build compiles in beside the hub, each on
`et-ws-wasi-runner`'s board runner. A runner's websocket is a link into the hub in the same firmware rather than a
socket, and the guests on one board share their storage buckets.

## Building

The build bakes in the network to join, from `WIFI_NETWORK` and `WIFI_PASSWORD`.

The Pico 2 W build also bakes in the CYW43439 radio's firmware. Its build task fetches the radio's `43439A0.bin`,
`43439A0_clm.bin` and `nvram_rp2040.bin` from the embassy repository's `cyw43-firmware` directory into
`target/cyw43-firmware`, and points `CYW43_FIRMWARE_DIR` at them. Infineon licenses those blobs separately from this
project, so they are not part of it. The ESP32-C6's radio blobs come with the `esp-radio` crate.

It serves no modules unless told which to build in, through the variables the `std` server reads, each optional:

- `MODULES_PATHS`: the module directories, comma-separated and absolute. Each is built in from its `pkg/` when it has
  one, otherwise from the directory itself: every file its `package.json` lists under `files`, or every file there.
- `MODULES_ROOT`: the name of the module served at `/`, one of those.
- `MODULES_ELSEWHERE` and `MODULES_ELSEWHERE_URL`: module names too big for a board's 4 MiB of flash, listed with the
  rest at `/modules/`, and the base URL a request for one of their files is redirected to.
- `RUNNER_GUESTS`: the runners to start beside the hub, comma-separated `runner=package` pairs, each naming a module
  `MODULES_PATHS` builds in whose `main` is a WebAssembly component. The build compiles each for the board, and
  refuses one that imports what a board does not host or guests needing more memory than the board holds for them,
  which is 6 MiB of the Pico Plus 2 W's PSRAM and none on the other boards. The guests start with what they declare,
  and the same 6 MiB bounds what they grow to as they run.

`et-cli generate-deployment --output-type firmware-pico2`, `firmware-esp32c6` or `firmware-pico-plus2` writes the
tasks that set all of these for a scenario.

### Raspberry Pi Pico 2 W

```sh
MISE_ENV=rust mise run build-firmware-pico2w
```

The firmware is `target/thumbv8m.main-none-eabihf/release/et-ws-server-firmware`, an ELF. Load it with picotool
while the board is in BOOTSEL mode, or with probe-rs over a debug probe, which also shows its defmt log:

```sh
picotool load -x -t elf target/thumbv8m.main-none-eabihf/release/et-ws-server-firmware
```

```sh
probe-rs run --chip RP235x target/thumbv8m.main-none-eabihf/release/et-ws-server-firmware
```

### Pimoroni Pico Plus 2 W

```sh
MISE_ENV=rust mise run build-firmware-pico-plus2w
```

The firmware is the same ELF path, and loads the same ways as the Pico 2 W's. Its log also shows each runner's guest
logging as it runs, and how it ended.

### ESP32-C6

```sh
MISE_ENV=rust mise run build-firmware-esp32c6
```

The firmware is `target/riscv32imac-unknown-none-elf/release/et-ws-server-firmware`, an ELF. espflash writes it to
the board over USB, with esp-hal's bootloader before it, then shows its defmt log:

```sh
espflash flash --monitor --log-format defmt target/riscv32imac-unknown-none-elf/release/et-ws-server-firmware
```

Either board's log names the address it serves on, as `ws://<address>:8080/ws`.

## Serving from the host

Built for a target with an operating system, the binary is the same hub serving the same modules and running the same
runners from the host over tokio, read from the same `MODULES_*` and `RUNNER_GUESTS` at build time, so what a board
would do can be explored without one. It listens on the address its one argument names, or on port 8080 on every
interface, and prints where:

```sh
cargo run -p et-int-ws-server-firmware -- 127.0.0.1:8080
```

The package's test starts it on a free port, fetches every module it lists, and waits for every runner it was built
with to join the hub, so run with the `MODULES_*` and `RUNNER_GUESTS` a scenario's deployment sets, as its
`firmware-host-test` task does, it checks what a board would do.
