from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.evicted_agent import EvictedAgent
from ...types import Response


def _get_kwargs(
    agent_id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": f"/agents/{quote(str(agent_id), safe='')}",
    }

    return _kwargs


def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Any | EvictedAgent | None:
    if response.status_code == 200:
        response_200 = EvictedAgent.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = cast(Any, None)
        return response_401

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 503:
        response_503 = cast(Any, None)
        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[Any | EvictedAgent]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    agent_id: str,
    *,
    client: AuthenticatedClient,
) -> Response[Any | EvictedAgent]:
    """Evict an agent: block the address it last connected from, and drop the messages it queued.

     The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct
    messages
    the agent sent that are still waiting for their recipients are dropped, so nothing it queued is
    delivered after
    it is gone.

    Args:
        agent_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | EvictedAgent]
    """

    kwargs = _get_kwargs(
        agent_id=agent_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    agent_id: str,
    *,
    client: AuthenticatedClient,
) -> Any | EvictedAgent | None:
    """Evict an agent: block the address it last connected from, and drop the messages it queued.

     The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct
    messages
    the agent sent that are still waiting for their recipients are dropped, so nothing it queued is
    delivered after
    it is gone.

    Args:
        agent_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | EvictedAgent
    """

    return sync_detailed(
        agent_id=agent_id,
        client=client,
    ).parsed


async def asyncio_detailed(
    agent_id: str,
    *,
    client: AuthenticatedClient,
) -> Response[Any | EvictedAgent]:
    """Evict an agent: block the address it last connected from, and drop the messages it queued.

     The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct
    messages
    the agent sent that are still waiting for their recipients are dropped, so nothing it queued is
    delivered after
    it is gone.

    Args:
        agent_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | EvictedAgent]
    """

    kwargs = _get_kwargs(
        agent_id=agent_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    agent_id: str,
    *,
    client: AuthenticatedClient,
) -> Any | EvictedAgent | None:
    """Evict an agent: block the address it last connected from, and drop the messages it queued.

     The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct
    messages
    the agent sent that are still waiting for their recipients are dropped, so nothing it queued is
    delivered after
    it is gone.

    Args:
        agent_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | EvictedAgent
    """

    return (
        await asyncio_detailed(
            agent_id=agent_id,
            client=client,
        )
    ).parsed
