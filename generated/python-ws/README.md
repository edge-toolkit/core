# et_ws — Pydantic client for the Edge Toolkit WS protocol

`et_ws/messages.py` is regenerated from `edge_toolkit::ws::{ClientMessage, ServerMessage}` via
`mise run gen-python-ws`. This README and `pyproject.toml` are checked in by hand.

## Build

```sh
mise run build-et-ws-wheel
```

Produces `dist/et_ws-<version>-py3-none-any.whl`. Pyodide consumers install it via
`micropip.install` from a URL served by the ws-server.
