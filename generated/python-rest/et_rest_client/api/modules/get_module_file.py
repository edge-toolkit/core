from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...types import Response


def _get_kwargs(
    name: str,
    path: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/modules/{name}/{path}".format(
            name=quote(str(name), safe=""),
            path=quote(str(path), safe=""),
        ),
    }

    return _kwargs


def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Any | None:
    if response.status_code == 200:
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
    name: str,
    path: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[Any]:
    """Fetch a file from a module's bundled static assets.

     `path` is resolved relative to the module's bundle root; an unknown module or missing file returns
    404.

    Both path parameters can themselves contain `/`. A module is served under the name its
    `package.json`
    declares, which carries an owner scope (`@scope/name`) for anything published to a registry, and
    `path`
    addresses sub-directories of the bundle. A client that percent-encodes each parameter as one path
    segment
    turns those slashes into `%2F` and asks for something no server serves, so build the request path
    rather
    than passing the values through a per-segment encoder.

    Args:
        name (str):
        path (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        name=name,
        path=path,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


async def asyncio_detailed(
    name: str,
    path: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[Any]:
    """Fetch a file from a module's bundled static assets.

     `path` is resolved relative to the module's bundle root; an unknown module or missing file returns
    404.

    Both path parameters can themselves contain `/`. A module is served under the name its
    `package.json`
    declares, which carries an owner scope (`@scope/name`) for anything published to a registry, and
    `path`
    addresses sub-directories of the bundle. A client that percent-encodes each parameter as one path
    segment
    turns those slashes into `%2F` and asks for something no server serves, so build the request path
    rather
    than passing the values through a per-segment encoder.

    Args:
        name (str):
        path (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        name=name,
        path=path,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)
