# et-ws-pyo3-runner

A small, **generic** program that lets you write an edge-toolkit agent in **plain Python** and have it talk to
the WebSocket server -- without writing any Rust.

## Is it generic?

Yes. The runner contains no application logic of its own. It is a _host_: it connects to the ws-server, then
hands every incoming message to a Python module **you** choose, and sends back whatever that module produces.
The exact same binary runs the toy `echo` example and the `fanout` and `storage` test modules -- only the
chosen Python module differs.

## How it loads your module

You point it at a Python module by name, and tell it where to find that module on disk:

```sh
RUNNER_MODULE=echo                          # which Python module to load (required)
PYO3_PYTHONPATH=modules/pyo3/echo           # folders to import it (and its dependencies) from
WS_SERVER_URL=ws://127.0.0.1:8080/ws        # where the ws-server is (optional)
cargo run -p et-ws-pyo3-runner
```

[HELP.md](https://github.com/edge-toolkit/core/blob/main/services/ws-pyo3-runner/HELP.md) lists every environment
variable the runner reads, with its type and default.

On startup the runner embeds a Python interpreter, `import`s the module named by `RUNNER_MODULE`, and from then
on just calls functions on it as things happen. Your module keeps its own state in ordinary Python globals; the
runner never looks inside.

Or you don't tell it where, and let it fetch the module from the ws-server:

```sh
RUNNER_MODULE=et-ws-pyo3-math1              # a module the ws-server publishes
WS_SERVER_URL=ws://127.0.0.1:8080/ws
cargo run -p et-ws-pyo3-runner
```

A name that resolves nowhere on `sys.path` is fetched from the ws-server, which serves it as a static asset the
way it serves every other module: the runner reads the module's `package.json`, downloads the single file its
`main` field names, and compiles that source under the file's own stem. This is the shape the WASI and web
runners already have, and it is what lets a generated deployment name a pyo3 module without also arranging for
that module to be on the runner's filesystem -- see `modules/pyo3/math1/` for one.

The local path is tried first and is unchanged, so a module on `PYO3_PYTHONPATH` never causes a fetch. Only the
module itself arrives over the wire: whatever it imports still resolves through `sys.path`, so its third-party
dependencies come from mise's site-packages exactly as a local module's do.

## The contract (every function is optional)

Your module may define any of these top-level functions; the runner calls them at the right moments:

| Function                 | Called when                                                        |
| ------------------------ | ------------------------------------------------------------------ |
| `init(send, storage)`    | once, at startup                                                   |
| `on_connect(agent_id)`   | once, after the server assigns this agent an id                    |
| `on_text_frame(text)`    | a text message arrived; return a reply (`str`/`bytes`) or `None`   |
| `on_binary_frame(frame)` | a binary message arrived; return a reply (`bytes`/`str`) or `None` |
| `on_shutdown()`          | once, as the connection closes                                     |

`init` receives two helpers:

- **`send`** -- call `send.text(...)` / `send.binary(...)` to push messages out at any time (not only as a reply
  to an incoming one, and even from a background thread).
- **`storage`** -- call `storage.get(agent_id, key)` / `storage.put(key, data)` to read and write files the
  ws-server keeps for each agent.

That is the whole interface: a module is "just a Python file with some of those functions." The smallest example
is `modules/pyo3/echo/echo.py`.

## Persistent workers

Work that outlives any one message, such as reading a sensor, belongs on a thread `init` starts. Hooks run on a
thread of the runner's own and `storage` waits for the hub with the interpreter released, so the worker runs
beside them. It can `send` at any time, but `storage.put` raises until `on_connect` has given the agent its bucket,
so a worker that stores waits for that first. Make it a daemon thread and stop it from `on_shutdown`, and it never
holds up the runner's exit.

The modules that read Hiwonder's MentorPi robot work this way, each storing the latest of what it reads in its agent's
bucket:

- `pyo3-lidar1` reads the COIN-D6 lidar, as the WASI runner's `wasi-lidar1` does, from the port `SERIAL_PORTS_LIDAR`
  names.
- `pyo3-imu1` reads the IMU on the robot's controller board, from the port `SERIAL_PORTS_CONTROLLER` names.
- `pyo3-media1` captures the camera through OpenCV and the microphone through sounddevice, from the sources
  `MEDIA_CAMERA` and `MEDIA_MICROPHONE` name.

The two serial-port readers open their ports with `edge_toolkit.serial`, which takes a device, or `tcp://<host>:<port>`
for one a bridge such as ser2net serves. They import it from this repository's own source, so `PYO3_PYTHONPATH` names
`libs/python-edge-toolkit`; pyserial, OpenCV and sounddevice come from mise's site-packages once the `python` mise
env installs them. On the robot, stop Hiwonder's ROS 2 drivers first, since they hold the same devices.
