# edge-toolkit

[Edge Toolkit](https://github.com/edge-toolkit/core) is a framework for running workloads across a fleet of devices --
browser tabs, phones, native processes -- that all connect to one WebSocket hub. Agents register with the hub,
exchange messages through it, and keep per-agent files in its storage. A workload is a ws-module, and this package is
the code the Python ws-modules share, whether they run in the browser under Pyodide or natively.

## Install

```sh
pip install edge-toolkit
```

It brings in [`et-ws`](https://pypi.org/project/et-ws/), the Pydantic models of the hub's wire protocol.

## Client events

`client_event` wraps a module's data in the [CloudEvents 1.0](https://cloudevents.io/) envelope the hub expects of
an `et-client-event` message:

```python
from edge_toolkit.events import client_event

message = client_event("/modules/my-module", "app", "loaded", {"build": "1.2.3"})
frame = message.model_dump_json()
```

The event's `type` is `et.<capability>.<action>` -- `et.app.loaded` here -- and its `source` is the path the hub
serves the module from. Each call gets a fresh UUID `id` and the current UTC `time`.

## Serial ports

A native module reads a device such as a robot's lidar through `edge_toolkit.serial`. A port is named as the WASI
runner lends one, by a `SERIAL_PORTS_<NAME>` variable holding the device -- `/dev/ttyUSB0`, `COM3` -- or
`tcp://<host>:<port>` for a port a bridge such as ser2net serves:

```python
from edge_toolkit.serial import PortClosedError, lent_device, open_port

port = open_port(lent_device("lidar"), 230_400, timeout=0.1)
data = port.read(4096)  # empty when nothing arrived within the timeout
```

A device needs pyserial, which `pip install edge-toolkit[serial]` brings in. A read raises `PortClosedError` once the
port has closed.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/edge-toolkit/core/blob/main/LICENSE-APACHE)
or [MIT license](https://github.com/edge-toolkit/core/blob/main/LICENSE-MIT) at your option.
