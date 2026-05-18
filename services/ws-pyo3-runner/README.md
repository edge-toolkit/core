# et-ws-pyo3-runner

A small, **generic** program that lets you write an edge-toolkit agent in **plain Python** and have it talk to
the WebSocket server — without writing any Rust.

## Is it generic?

Yes. The runner contains no application logic of its own. It is a _host_: it connects to the ws-server, then
hands every incoming message to a Python module **you** choose, and sends back whatever that module produces.
The exact same binary runs the toy `echo` example and the `fanout` and `storage` test modules — only the
chosen Python module differs.

## How it loads your module

You point it at a Python module by name and tell it where to find that module on disk:

```sh
RUNNER_MODULE=echo                          # which Python module to load (required)
PYO3_PYTHONPATH=.../python                   # folders to import it (and its dependencies) from
WS_SERVER_URL=ws://127.0.0.1:8080/ws        # where the ws-server is (optional)
cargo run -p et-ws-pyo3-runner
```

On startup the runner embeds a Python interpreter, `import`s the module named by `RUNNER_MODULE`, and from then
on just calls functions on it as things happen. Your module keeps its own state in ordinary Python globals; the
runner never looks inside.

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

- **`send`** — call `send.text(...)` / `send.binary(...)` to push messages out at any time (not only as a reply
  to an incoming one, and even from a background thread).
- **`storage`** — call `storage.get(agent_id, key)` / `storage.put(key, data)` to read and write files the
  ws-server keeps for each agent.

That is the whole interface: a module is "just a Python file with some of those functions." The smallest example
is [`python/echo.py`](python/echo.py).
