# noqa: INP001 -- et-ws-pyo3-runner compiles this one file on its own; it is not a package
"""Read the COIN-D6 lidar on Hiwonder's MentorPi robots, keeping its latest revolution in the agent's storage.

The native-CPython twin of the wasi-lidar1 module, storing the same `lidar-scan.json` in its agent's bucket. `init`
starts a persistent worker thread, which opens the port lent as `lidar`, as the WASI runner lends it, from the variable
`SERIAL_PORTS_LIDAR`. Once `on_connect` has given the agent its bucket, the worker asks the lidar to scan and stores
each whole revolution it decodes, until the port closes or the runner shuts down.
"""

from __future__ import annotations

__all__ = ["init", "on_connect", "on_shutdown"]

import json
import logging
import threading
from typing import Any

from edge_toolkit.serial import PortClosedError, lent_device, open_port
from edge_toolkit.worker import persistent_workers

_logger = logging.getLogger(__name__)

PORT = "lidar"
SCAN_KEY = "lidar-scan.json"
BAUD_RATE = 230_400
START_SCAN = bytes([0xAA, 0x55, 0xF0, 0x0F])

_SYNC = bytes([0xAA, 0x55])
_HEAD_LEN = 10
_SAMPLE_LEN = 3
_FULL_TURN = 360 * 64
_READ_MAX = 4096
_READ_TIMEOUT_S = 0.1


class Scanner:
    """Turn the lidar's byte stream into whole revolutions, however the bytes are split as they arrive.

    Samples before the first packet to start a revolution belong to one only partly seen, so they are dropped; a
    revolution is complete when the next one starts.
    """

    def __init__(self) -> None:
        """Start with no bytes seen."""
        self._pending = bytearray()
        self._revolution: dict[str, Any] | None = None
        self.skipped = 0
        self.rejected = 0

    def push(self, data: bytes) -> list[dict[str, Any]]:
        """Return every revolution the bytes so far complete, `data` the latest of them."""
        self._pending += data
        scans: list[dict[str, Any]] = []
        for starts, frequency_dhz, samples in self._packets():
            if starts:
                if self._revolution is not None:
                    scans.append(self._revolution)
                self._revolution = {"frequency_dhz": frequency_dhz, "samples": samples}
            elif self._revolution is not None:
                self._revolution["samples"].extend(samples)
        return scans

    def _packets(self) -> list[tuple[bool, int, list[tuple[int, int, int, bool]]]]:
        packets: list[tuple[bool, int, list[tuple[int, int, int, bool]]]] = []
        while True:
            start = self._pending.find(_SYNC)
            if start < 0:
                # A last byte that could be the first of a header is kept until the next byte says whether it is.
                noise = len(self._pending) - int(self._pending.endswith(_SYNC[:1]))
                self._drop(noise)
                return packets
            self._drop(start)
            if len(self._pending) < _HEAD_LEN:
                return packets
            flags, count = self._pending[2], self._pending[3]
            first, last, checksum = (int.from_bytes(self._pending[at : at + 2], "little") for at in (4, 6, 8))
            if count == 0 or not first & 1 or not last & 1:
                self._reject()
                continue
            length = _HEAD_LEN + _SAMPLE_LEN * count
            if len(self._pending) < length:
                return packets
            body = bytes(self._pending[_HEAD_LEN:length])
            if _checksum(bytes(self._pending[2:8]), body) != checksum:
                self._reject()
                continue
            self._drop(length, counted=False)
            packets.append((bool(flags & 1), flags >> 1, _spread(first >> 1, last >> 1, body)))

    def _drop(self, count: int, *, counted: bool = True) -> None:
        if counted:
            self.skipped += count
        del self._pending[:count]

    def _reject(self) -> None:
        self.rejected += 1
        self._drop(1)


def _checksum(head: bytes, body: bytes) -> int:
    """XOR the header word, the head's three words after it, and each sample's first byte and remaining word."""
    total = int.from_bytes(_SYNC, "little")
    for at in range(0, len(head), 2):
        total ^= int.from_bytes(head[at : at + 2], "little")
    for at in range(0, len(body), _SAMPLE_LEN):
        total ^= body[at] ^ int.from_bytes(body[at + 1 : at + 3], "little")
    return total


def _spread(first: int, last: int, body: bytes) -> list[tuple[int, int, int, bool]]:
    """Decode `body`'s samples as (angle, distance in mm, intensity, high reflectivity), spaced from first to last."""
    count = len(body) // _SAMPLE_LEN
    span = (last + _FULL_TURN if last < first else last) - first
    samples: list[tuple[int, int, int, bool]] = []
    for index in range(count):
        low, middle, high = body[index * _SAMPLE_LEN : (index + 1) * _SAMPLE_LEN]
        offset = span * index // (count - 1) if count > 1 else 0
        distance = int.from_bytes(bytes([middle, high]), "little") >> 2
        samples.append(((first + offset) % _FULL_TURN, distance, ((middle & 0b11) << 6) | (low >> 2), bool(low & 1)))
    return samples


def scan_json(scan: dict[str, Any]) -> bytes:
    """Render a revolution as the JSON wasi-lidar1 stores, angles in degrees and the frequency in hertz."""
    samples = scan["samples"]
    return json.dumps(
        {
            "frequency_hz": scan["frequency_dhz"] / 10,
            "angle_deg": [angle / 64 for angle, _, _, _ in samples],
            "distance_mm": [distance for _, distance, _, _ in samples],
            "intensity": [intensity for _, _, intensity, _ in samples],
            "high_reflectivity": [flag for _, _, _, flag in samples],
        }
    ).encode("utf-8")


def _read_lidar(storage: Any, stopping: threading.Event) -> None:
    """Store each revolution the lidar sends, until its port closes or the runner shuts down."""
    device = lent_device(PORT)
    port = open_port(device, BAUD_RATE, _READ_TIMEOUT_S)
    stored = 0
    scanner = Scanner()
    try:
        port.write(START_SCAN)
        _logger.info("asked the lidar on %s to scan", device)
        while not stopping.is_set():
            for scan in scanner.push(port.read(_READ_MAX)):
                storage.put(SCAN_KEY, scan_json(scan))
                stored += 1
    except PortClosedError:
        _logger.info("the lidar's port closed")
    finally:
        port.close()
    _logger.info(
        "stored %d revolutions, with %d bytes skipped and %d packets rejected",
        stored,
        scanner.skipped,
        scanner.rejected,
    )


init, on_connect, on_shutdown = persistent_workers({"lidar": _read_lidar})
