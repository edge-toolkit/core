from __future__ import annotations

from enum import Enum
from typing import Any, Literal

from pydantic import BaseModel, Field, RootModel


class WsConnect(BaseModel):
    agent_id: str | None = None
    type: Literal["et-connect"]


class WsAlive(BaseModel):
    timestamp: str
    type: Literal["et-alive"]


class WsListAgents(BaseModel):
    type: Literal["et-list-agents"]


class WsSendAgentMessage(BaseModel):
    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    to_agent_id: str
    type: Literal["et-send-agent-message"]


class WsBroadcastMessage(BaseModel):
    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    type: Literal["et-broadcast-message"]


class WsMessageAck(BaseModel):
    message_id: str
    type: Literal["et-message-ack"]


class WsInvalid(BaseModel):
    detail: str
    message_id: str | None = None
    type: Literal["et-invalid"]


class WsClientEvent(BaseModel):
    action: str
    capability: str
    details: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    type: Literal["et-client-event"]


class WsResponse(BaseModel):
    message: str
    type: Literal["et-response"]


class AgentConnectionState(Enum):
    connected = "connected"
    disconnected = "disconnected"


class AgentSummary(BaseModel):
    agent_id: str
    last_known_ip: str | None = None
    state: AgentConnectionState


class ConnectStatus(Enum):
    assigned = "assigned"
    reconnected = "reconnected"


class MessageDeliveryStatus(Enum):
    delivered = "delivered"
    queued = "queued"
    acknowledged = "acknowledged"
    broadcast = "broadcast"


class MessageScope(Enum):
    direct = "direct"
    broadcast = "broadcast"


class WsConnectAck(BaseModel):
    agent_id: str
    status: ConnectStatus
    type: Literal["et-connect-ack"]


class WsListAgentsResponse(BaseModel):
    agents: list[AgentSummary]
    type: Literal["et-list-agents-response"]


class WsAgentMessage(BaseModel):
    from_agent_id: str
    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    message_id: str
    scope: MessageScope
    server_received_at: str
    type: Literal["et-agent-message"]


class WsMessageStatus(BaseModel):
    detail: str
    message_id: str | None = None
    status: MessageDeliveryStatus
    type: Literal["et-message-status"]


class WsMessage(
    RootModel[
        WsConnect
        | WsConnectAck
        | WsAlive
        | WsListAgents
        | WsListAgentsResponse
        | WsSendAgentMessage
        | WsBroadcastMessage
        | WsAgentMessage
        | WsMessageAck
        | WsMessageStatus
        | WsInvalid
        | WsClientEvent
        | WsResponse
    ]
):
    root: (
        WsConnect
        | WsConnectAck
        | WsAlive
        | WsListAgents
        | WsListAgentsResponse
        | WsSendAgentMessage
        | WsBroadcastMessage
        | WsAgentMessage
        | WsMessageAck
        | WsMessageStatus
        | WsInvalid
        | WsClientEvent
        | WsResponse
    ) = Field(..., title="WsMessage")
