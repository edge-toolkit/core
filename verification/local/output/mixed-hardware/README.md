# mixed-hardware

This directory contains generated deployment configs for the `mixed-hardware` scenario.
Files: `mise.toml`, `dekit.yaml`, `process-compose.yaml`.

The scenario exposes these workflow modules: pyo3-math1, wasi-math1-sender.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

## Lock The Tools

Before the first run, lock the tools `mise.toml` names:

```bash
mise lock
```

Commit `mise.lock` alongside the deployment, together with the `.mise/locks/` directory when
mise writes one -- it holds the dependency graph of each npm tool, which `mise.lock` refers to.
The two record the version and checksum every `latest` in `mise.toml` resolved to, for each of
mise's common platforms, so every later install -- on any machine -- fetches the same binaries
rather than whatever is newest that day. Move to newer releases deliberately, with
`mise lock --bump`, and commit the result.

## Run With Mise

The hub runs on `hub-board`, a Raspberry Pi Pico 2 W,
as firmware the section loading it describes. The board runs no OpenObserve, so there is no UI to
open. Each runner on another machine reaches the hub across the network, and starts as that
machine's share below.

### Run Across Machines With Mise

Copy this directory to each machine and start that machine's share of the scenario there.
`hub-board` is a Raspberry Pi Pico 2 W running `ws-server` as firmware,
and the runners on every other machine reach the hub at `et-hub-board.local`, which the network the board
joins has to resolve to it. Its share is loading the firmware, which it then runs from power-on.

On `hub-board`:

```bash
mise run firmware-flash-hub-board
```

On `edge`:

```bash
mise run machine-edge
```

On `runner-board`:

```bash
mise run firmware-flash-runner-board
```

## Run With dekit

The hub runs on `hub-board`, a Raspberry Pi Pico 2 W,
as firmware the section loading it describes. The board runs no OpenObserve, so there is no UI to
open. Each runner on another machine reaches the hub across the network, and starts as that
machine's share below.

### Run Across Machines With dekit

Copy this directory to each machine and start that machine's share there. Every task is tagged
with its machine and none starts on `dekit up` alone, so `dekit up` on any machine starts nothing
until its tag is started.
`hub-board` is a Raspberry Pi Pico 2 W running `ws-server` as firmware,
and the runners on every other machine reach the hub at `et-hub-board.local`, which the network the board
joins has to resolve to it. Its share is loading the firmware, which it then runs from power-on.

On `hub-board`:

```bash
mise run firmware-flash-hub-board
```

On `edge`:

```bash
dekit up
dekit start +edge
```

On `runner-board`:

```bash
mise run firmware-flash-runner-board
```

## Run With process-compose

The hub runs on `hub-board`, a Raspberry Pi Pico 2 W,
as firmware the section loading it describes. The board runs no OpenObserve, so there is no UI to
open. Each runner on another machine reaches the hub across the network, and starts as that
machine's share below.

### Run Across Machines With process-compose

Copy this directory to each machine and start only that machine's namespace there.
`hub-board` is a Raspberry Pi Pico 2 W running `ws-server` as firmware,
and the runners on every other machine reach the hub at `et-hub-board.local`, which the network the board
joins has to resolve to it. Its share is loading the firmware, which it then runs from power-on.

On `hub-board`:

```bash
mise run firmware-flash-hub-board
```

On `edge`:

```bash
process-compose up --no-server -f process-compose.yaml -n edge
```

On `runner-board`:

```bash
mise run firmware-flash-runner-board
```

## Load The Hub Onto `hub-board`

The board runs the hub alone and serves this scenario's modules from its 4 MiB of flash.
Build those modules first, since the firmware takes each one's built package.
Set `WIFI_NETWORK` and `WIFI_PASSWORD` to the Wi-Fi network the board joins.
Then hold the board's BOOTSEL button while plugging it in.
From this directory, build the firmware and load it with:

```bash
mise run firmware-flash-hub-board
```

To check what the board will serve without one, serve the same modules from this host and fetch
each, as a page would:

```bash
mise run firmware-host-test-hub-board
```

The board takes an address over DHCP and serves the page at port 8080 of it. npm packages the
modules use that are too big for its flash, such as onnxruntime-web, are not built in: the board
redirects a request for one to the same path under `https://cdn.jsdelivr.net/npm`, which the browser has to be
able to reach.

## Load The Runners Onto `runner-board`

`runner-board` is a Pimoroni Pico Plus 2 W running each wasi runner the scenario places on it, compiled into the
firmware from its module's component, and no hub: the runners join the one at `ws://et-hub-board.local:8080/ws`.
Build those modules first, since the firmware takes each one's built package.
Set `WIFI_NETWORK` and `WIFI_PASSWORD` to the Wi-Fi network the board joins.
Then hold the board's BOOTSEL button while plugging it in.
From this directory, build the firmware and load it with:

```bash
mise run firmware-flash-runner-board
```
