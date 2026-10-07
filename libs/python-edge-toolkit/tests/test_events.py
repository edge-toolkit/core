"""The CloudEvents envelope `client_event` wraps a module's data in."""

import json
from datetime import datetime, timedelta

from edge_toolkit.events import client_event


def test_a_client_event_wraps_its_data_in_a_cloudevent() -> None:
    message = json.loads(client_event("/modules/test", "app", "loaded", {"build": "test"}).model_dump_json())

    assert message["type"] == "et-client-event"
    event = message["event"]
    assert event["specversion"] == "1.0"
    assert event["type"] == "et.app.loaded"
    assert event["source"] == "/modules/test"
    assert event["data"] == {"build": "test"}
    assert event["time"].endswith("Z")
    # Before Python 3.11 `fromisoformat` rejects a trailing `Z`, so it is spelled as the offset it stands for.
    assert datetime.fromisoformat(event["time"].removesuffix("Z") + "+00:00").utcoffset() == timedelta(0)


def test_each_client_event_gets_its_own_id() -> None:
    first = client_event("/modules/test", "app", "loaded", {})
    second = client_event("/modules/test", "app", "loaded", {})

    assert first.event.id != second.event.id
