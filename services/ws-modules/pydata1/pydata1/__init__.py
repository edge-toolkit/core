"""pydata1: Python implementation of the data1 workflow."""

from datetime import datetime, timezone

import pyodide_http
from et_rest_client import Client
from et_rest_client.api.storage import get_file, put_file
from et_rest_client.types import File


async def run(agent_id, base_url, sleep_ms, log, set_status) -> None:
    """Execute the data1 workflow: store, fetch, verify."""
    # httpx ships its own transport stack; in Pyodide that stack has no
    # network access. `pyodide_http.patch_all()` swaps httpx's transports
    # for ones that dispatch through the browser's fetch(), letting the
    # generated client work unmodified.
    pyodide_http.patch_all()

    log("pydata1: entered run()")

    filename = "test_data.txt"
    test_content = f"Hello from pydata1 at {datetime.now(timezone.utc).isoformat()}!"

    msg = f"pydata1: storing data to /storage/{agent_id}/{filename}"
    log(msg)
    set_status(msg)
    async with Client(base_url=base_url) as c:
        await put_file.asyncio_detailed(
            agent_id,
            filename,
            client=c,
            body=File(payload=test_content.encode("utf-8")),
        )

        msg = f"pydata1: fetching data from /storage/{agent_id}/{filename}"
        log(msg)
        set_status(msg)
        response = await get_file.asyncio_detailed(agent_id, filename, client=c)

    retrieved = response.content.decode("utf-8")
    if retrieved == test_content:
        msg = "pydata1: VERIFICATION SUCCESS - data matches!"
        log(msg)
        set_status(msg)
    else:
        msg = f"pydata1: VERIFICATION FAILURE\nSent: {test_content}\nGot: {retrieved}"
        log(msg)
        set_status(msg)
        raise RuntimeError("Data mismatch")

    await sleep_ms(2000)
    log("pydata1: workflow complete")
    set_status("pydata1: workflow complete")
