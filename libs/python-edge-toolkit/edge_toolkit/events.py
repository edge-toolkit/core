"""CloudEvents envelopes for the et-client-event message."""

from __future__ import annotations

import uuid
from datetime import datetime, timezone

from et_ws.messages import CloudEvent, WsClientEvent

CLOUDEVENTS_SPEC_VERSION = "1.0"


def client_event(source: str, capability: str, action: str, data: object) -> WsClientEvent:
    """Wrap `data` as a CloudEvent of type `et.<capability>.<action>` from the module served at `source`."""
    return WsClientEvent(
        type="et-client-event",
        event=CloudEvent(
            data=data,
            id=str(uuid.uuid4()),
            source=source,
            specversion=CLOUDEVENTS_SPEC_VERSION,
            time=datetime.now(timezone.utc).isoformat(),
            type=f"et.{capability}.{action}",
        ),
    )
