# noqa: INP001 -- et-ws-pyo3-runner compiles this one file on its own; it is not a package
"""Read the IMU on Hiwonder's MentorPi robots, keeping its latest reading in the agent's storage.

The IMU is on the robot's controller board, Hiwonder's RRC Lite, which reports it over the serial port lent as
`controller`, from the variable `SERIAL_PORTS_CONTROLLER`, at 1,000,000 baud. The board sends frames of `AA 55`, a
function code, the data's length, the data, and the Dallas/Maxim CRC-8 of the code, length and data; function 7 is an
IMU reading, six little-endian floats of acceleration in g and rotation in degrees a second, each along x, y and z.

`init` starts a persistent worker thread. Once `on_connect` has given the agent its bucket, the worker stores the latest
reading as `imu-reading.json`, at most ten times a second, in metres per second squared and radians a second, until
the port closes or the runner shuts down.
"""

from __future__ import annotations

__all__ = ["init", "on_connect", "on_shutdown"]

import json
import logging
import math
import struct
import threading
import time
from typing import Any

from edge_toolkit.serial import PortClosedError, lent_device, open_port
from edge_toolkit.worker import persistent_workers

_logger = logging.getLogger(__name__)

PORT = "controller"
READING_KEY = "imu-reading.json"
BAUD_RATE = 1_000_000
STANDARD_GRAVITY = 9.80665

_SYNC = bytes([0xAA, 0x55])
_FUNCTION_IMU = 7
# The board's function codes run from 0 to 11; a header naming another starts no frame.
_FUNCTION_COUNT = 12
_HEAD_LEN = 4
_IMU_LEN = 24
_READ_MAX = 4096
_READ_TIMEOUT_S = 0.05
_STORE_INTERVAL_S = 0.1


def crc8(data: bytes) -> int:
    """Return the Dallas/Maxim CRC-8 of `data`, which the board checks each frame with."""
    crc = 0
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0x8C if crc & 1 else crc >> 1
    return crc


class FrameDecoder:
    """Turn the board's byte stream into frames, however the bytes are split as they arrive."""

    def __init__(self) -> None:
        """Start with no bytes seen."""
        self._pending = bytearray()
        self.rejected = 0

    def push(self, data: bytes) -> list[tuple[int, bytes]]:
        """Return the function code and data of every frame the bytes so far complete, `data` the latest of them."""
        self._pending += data
        frames: list[tuple[int, bytes]] = []
        while True:
            start = self._pending.find(_SYNC)
            if start < 0:
                # A last byte that could be the first of a header is kept until the next byte says whether it is.
                del self._pending[: len(self._pending) - int(self._pending.endswith(_SYNC[:1]))]
                return frames
            del self._pending[:start]
            if len(self._pending) < _HEAD_LEN:
                return frames
            function, length = self._pending[2], self._pending[3]
            if function >= _FUNCTION_COUNT:
                self._reject()
                continue
            end = _HEAD_LEN + length
            if len(self._pending) <= end:
                return frames
            if crc8(bytes(self._pending[2:end])) != self._pending[end]:
                self._reject()
                continue
            frames.append((function, bytes(self._pending[_HEAD_LEN:end])))
            del self._pending[: end + 1]

    def _reject(self) -> None:
        self.rejected += 1
        del self._pending[:1]


def reading_json(data: bytes) -> bytes:
    """Render an IMU frame's data as the JSON stored, in metres per second squared and radians a second."""
    ax, ay, az, gx, gy, gz = struct.unpack("<6f", data)
    return json.dumps(
        {
            "acceleration_m_s2": [value * STANDARD_GRAVITY for value in (ax, ay, az)],
            "angular_velocity_rad_s": [math.radians(value) for value in (gx, gy, gz)],
        }
    ).encode("utf-8")


def _read_imu(storage: Any, stopping: threading.Event) -> None:
    """Store the board's latest IMU reading, until its port closes or the runner shuts down."""
    device = lent_device(PORT)
    port = open_port(device, BAUD_RATE, _READ_TIMEOUT_S)
    _logger.info("reading the IMU from the controller board on %s", device)
    decoder = FrameDecoder()
    latest = None
    stored_at = -math.inf
    stored = 0
    try:
        while not stopping.is_set():
            for function, data in decoder.push(port.read(_READ_MAX)):
                if function == _FUNCTION_IMU and len(data) == _IMU_LEN:
                    latest = data
            now = time.monotonic()
            if latest is not None and now - stored_at >= _STORE_INTERVAL_S:
                storage.put(READING_KEY, reading_json(latest))
                latest = None
                stored_at = now
                stored += 1
    except PortClosedError:
        _logger.info("the controller board's port closed")
    finally:
        port.close()
    _logger.info("stored %d IMU readings, with %d frames rejected", stored, decoder.rejected)


init, on_connect, on_shutdown = persistent_workers({"imu": _read_imu})
