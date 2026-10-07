"""The CloudEvents envelope `client_event` wraps a module's data in."""

import json

from edge_toolkit.events import CLOUDEVENTS_SPEC_VERSION, client_event


def test_a_client_event_wraps_its_data_in_a_cloudevent() -> None:
    message = json.loads(client_event("/modules/test", "app", "loaded", {"build": "test"}).model_dump_json())

    assert message["type"] == "et-client-event"
    event = message["event"]
    assert event["specversion"] == CLOUDEVENTS_SPEC_VERSION == "1.0"
    assert event["type"] == "et.app.loaded"
    assert event["source"] == "/modules/test"
    assert event["data"] == {"build": "test"}
    assert event["time"].endswith("+00:00")


def test_each_client_event_gets_its_own_id() -> None:
    first = client_event("/modules/test", "app", "loaded", {})
    second = client_event("/modules/test", "app", "loaded", {})

    assert first.event.id != second.event.id
