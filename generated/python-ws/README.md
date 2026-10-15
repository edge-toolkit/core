# et-ws

Pydantic v2 models for the WebSocket protocol of [Edge Toolkit](https://github.com/edge-toolkit/core), a framework
for running workloads across a fleet of devices that all connect to one WebSocket hub. Every message an agent may
send to the hub is a model here, so a Python agent builds its frames with validation rather than by hand.

## Install

```sh
pip install et-ws
```

## Usage

`ClientMessage` is the union of everything an agent sends; each variant is its own model, discriminated by `type`:

```python
from et_ws.messages import ClientMessage, WsBroadcastMessage

frame = WsBroadcastMessage(type="et-broadcast-message", message={"hello": "world"}).model_dump_json()
parsed = ClientMessage.model_validate_json(frame)
```

The full message catalogue, including the hub's replies, is the
[AsyncAPI specification](https://github.com/edge-toolkit/core/blob/main/generated/specs/ws.yaml).

## Regenerating

`et_ws/messages.py` is generated from the Rust `ClientMessage` in `libs/edge-toolkit/src/ws.rs` by
`mise run gen:python-ws`. This README and `pyproject.toml` are maintained by hand.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/edge-toolkit/core/blob/main/LICENSE-APACHE)
or [MIT license](https://github.com/edge-toolkit/core/blob/main/LICENSE-MIT) at your option.
