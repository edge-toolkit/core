from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...types import File, Response


def _get_kwargs(
    agent_id: str,
    filename: str,
    *,
    body: File,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/storage/{agent_id}/{filename}".format(
            agent_id=quote(str(agent_id), safe=""),
            filename=quote(str(filename), safe=""),
        ),
    }

    _kwargs["content"] = body.payload
    headers["Content-Type"] = "application/octet-stream"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Any | None:
    if response.status_code == 200:
        return None

    if response.status_code == 400:
        return None

    if response.status_code == 404:
        return None

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[Any]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    agent_id: str,
    filename: str,
    *,
    client: AuthenticatedClient | Client,
    body: File,
) -> Response[Any]:
    """Upload a file to an agent's storage bucket.

     Only the agent that owns the bucket may write to it (the agent must
    currently be connected); the path component must be a single
    filename, not a nested path.

    Args:
        agent_id (str):
        filename (str):
        body (File): Phantom type used to label binary request/response bodies as
            `string`/`binary`.

            Never constructed at runtime; only exists under the `openapi-spec` feature
            so the `utoipa::ToSchema` derive has something to attach to.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        agent_id=agent_id,
        filename=filename,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


async def asyncio_detailed(
    agent_id: str,
    filename: str,
    *,
    client: AuthenticatedClient | Client,
    body: File,
) -> Response[Any]:
    """Upload a file to an agent's storage bucket.

     Only the agent that owns the bucket may write to it (the agent must
    currently be connected); the path component must be a single
    filename, not a nested path.

    Args:
        agent_id (str):
        filename (str):
        body (File): Phantom type used to label binary request/response bodies as
            `string`/`binary`.

            Never constructed at runtime; only exists under the `openapi-spec` feature
            so the `utoipa::ToSchema` derive has something to attach to.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        agent_id=agent_id,
        filename=filename,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)
