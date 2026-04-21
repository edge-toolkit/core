"""pydata1: Python implementation of the data1 workflow."""

import json
from datetime import datetime, timezone


async def run(
    ws_send, wait_for_response, put_file, get_file, sleep_ms, log, set_status
) -> None:
    """Execute the data1 workflow: connect, store, fetch, verify."""
    log("pydata1: entered run()")

    filename = "test_data.txt"
    test_content = f"Hello from pydata1 at {datetime.now(timezone.utc).isoformat()}!"

    # 1. Request Store URL
    log("pydata1: requesting store URL")
    ws_send(json.dumps({"type": "store_file", "filename": filename}))
    store_response = await wait_for_response("PUT to ")
    store_url = store_response.replace("PUT to ", "")

    # 2. Perform PUT
    msg = f"pydata1: storing data to {store_url}"
    log(msg)
    set_status(msg)
    await put_file(store_url, test_content)

    # 3. Request Fetch URL
    log("pydata1: requesting fetch URL")
    ws_send(json.dumps({"type": "fetch_file", "filename": filename}))
    fetch_response = await wait_for_response("GET from ")
    fetch_url = fetch_response.replace("GET from ", "")

    # 4. Perform GET and Verify
    msg = f"pydata1: fetching data from {fetch_url}"
    log(msg)
    set_status(msg)
    retrieved = await get_file(fetch_url)

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
