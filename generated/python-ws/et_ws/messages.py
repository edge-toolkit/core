from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field, RootModel


class WsConnect(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    agent_id: str | None = None
    type: Literal["et-connect"]


class WsAlive(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    timestamp: str
    type: Literal["et-alive"]


class WsListAgents(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    type: Literal["et-list-agents"]


class WsSendAgentMessage(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    to_agent_id: str
    type: Literal["et-send-agent-message"]


class WsBroadcastMessage(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    message: Any = Field(..., description="Arbitrary JSON value (opaque to the protocol)")
    type: Literal["et-broadcast-message"]


class WsMessageAck(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    message_id: str
    type: Literal["et-message-ack"]


class WsClientRelayText(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    content: str
    type: Literal["et-relay-text"]


class ContentItem(RootModel[int]):
    root: int = Field(..., ge=0, le=255)


class WsClientRelayBinary(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    content: list[ContentItem] = Field(..., description="Byte array (uint8)")
    type: Literal["et-relay-binary"]


class CloudEvent(BaseModel):
    """
    A `CloudEvents` 1.0 event in the JSON event format, as `et-client-event` carries it.

    `data` is always JSON, so `datacontenttype` is omitted: JSON events imply `application/json`.

    `time` is optional in the specification but required here: every producer has a clock, and the hub logs it.
    """

    data: Any = Field(..., description="The event payload.")
    id: str = Field(..., description="Unique among the events of one `source`.")
    source: str = Field(..., description="The producing module's served path, `/modules/<package name>`.")
    specversion: str = Field(..., description="The `CloudEvents` specification version, `1.0`.")
    time: str = Field(..., description="When the event happened, as an RFC 3339 timestamp.")
    type: str = Field(..., description="`et.<capability>.<action>`.")


class WsClientEvent(BaseModel):
    """
    Messages a client is allowed to SEND to the server.

    Split from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server's
    inbound match arms can be exhaustive without an "unexpected server-originated message" trap.
    """

    event: CloudEvent
    type: Literal["et-client-event"]


class ClientMessage(
    RootModel[
        WsConnect
        | WsAlive
        | WsListAgents
        | WsSendAgentMessage
        | WsBroadcastMessage
        | WsMessageAck
        | WsClientEvent
        | WsClientRelayText
        | WsClientRelayBinary
    ]
):
    root: (
        WsConnect
        | WsAlive
        | WsListAgents
        | WsSendAgentMessage
        | WsBroadcastMessage
        | WsMessageAck
        | WsClientEvent
        | WsClientRelayText
        | WsClientRelayBinary
    ) = Field(
        ...,
        description='Messages a client is allowed to SEND to the server.\n\nSplit from [`ServerMessage`] so the type system rejects client code constructing a `ConnectAck`, and so the server\'s\ninbound match arms can be exhaustive without an "unexpected server-originated message" trap.',
        title="ClientMessage",
    )
