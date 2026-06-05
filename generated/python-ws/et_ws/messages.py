from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field, RootModel


class WsClientConnect(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    agent_id: str | None = None
    type: Literal["et-connect"]


class WsClientAlive(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    timestamp: str
    type: Literal["et-alive"]


class WsClientListAgents(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    type: Literal["et-list-agents"]


class WsClientSendAgentMessage(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    to_agent_id: str
    type: Literal["et-send-agent-message"]


class WsClientBroadcastMessage(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    type: Literal["et-broadcast-message"]


class WsClientMessageAck(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    message_id: str
    type: Literal["et-message-ack"]


class WsClientClientEvent(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    action: str
    capability: str
    details: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    type: Literal["et-client-event"]


class WsClientRelayText(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    content: str
    type: Literal["et-relay-text"]


class ContentItem(RootModel[int]):
    root: int = Field(..., ge=0, le=255)


class WsClientRelayBinary(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code
    constructing a `ConnectAck`, and so the server's inbound match arms
    can be exhaustive without an "unexpected server-originated message"
    trap.

    Wire tags are unchanged from the pre-split `WsMessage`; the JSON
    envelope is identical to what's documented in the `AsyncAPI` spec.
    """

    content: list[ContentItem] = Field(..., description="Byte array (uint8)")
    type: Literal["et-relay-binary"]


class ClientMessage(
    RootModel[
        WsClientConnect
        | WsClientAlive
        | WsClientListAgents
        | WsClientSendAgentMessage
        | WsClientBroadcastMessage
        | WsClientMessageAck
        | WsClientClientEvent
        | WsClientRelayText
        | WsClientRelayBinary
    ]
):
    root: (
        WsClientConnect
        | WsClientAlive
        | WsClientListAgents
        | WsClientSendAgentMessage
        | WsClientBroadcastMessage
        | WsClientMessageAck
        | WsClientClientEvent
        | WsClientRelayText
        | WsClientRelayBinary
    ) = Field(
        ...,
        description="Messages a client is allowed to SEND to the server.\n\nSplit from [`ServerMessage`] so the type system rejects client code\nconstructing a `ConnectAck`, and so the server's inbound match arms\ncan be exhaustive without an \"unexpected server-originated message\"\ntrap.\n\nWire tags are unchanged from the pre-split `WsMessage`; the JSON\nenvelope is identical to what's documented in the `AsyncAPI` spec.",
        title="ClientMessage",
    )
