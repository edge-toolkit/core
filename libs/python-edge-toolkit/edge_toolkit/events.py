"""CloudEvents envelopes for the et-client-event message."""

from __future__ import annotations

import uuid
from datetime import datetime, timezone

from et_ws.messages import CloudEvent, SpecVersion, WsClientEvent


def client_event(source: str, capability: str, action: str, data: object) -> WsClientEvent:
    """Wrap `data` as a CloudEvent of type `et.<capability>.<action>` from the module served at `source`."""
    return WsClientEvent(
        type="et-client-event",
        event=CloudEvent(
            data=data,
            id=str(uuid.uuid4()),
            source=source,
            specversion=SpecVersion.field_1_0,
            time=datetime.now(timezone.utc),
            type=f"et.{capability}.{action}",
        ),
    )
