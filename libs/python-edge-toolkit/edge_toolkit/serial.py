"""Serial ports, for a native Python ws-module that reads a device such as a robot's lidar or controller board.

A port is named the way the WASI runner lends one, by a `SERIAL_PORTS_<NAME>` variable whose value is the device --
`/dev/ttyUSB0`, `COM3` -- or `tcp://<host>:<port>` for a port a bridge such as ser2net serves over TCP. A device is
opened through pyserial, which the `serial` extra installs; a bridge needs nothing beyond the standard library.
"""

from __future__ import annotations

import os
import socket

TCP_PREFIX = "tcp://"

# How long a bridge has to accept the connection: far longer than one read waits, since a bridge on another host or
# a loaded one can take more than a read's fraction of a second to answer, and failing that ends the module's worker.
CONNECT_TIMEOUT_S = 5.0


class PortClosedError(Exception):
    """The port closed: a bridge hung up, or the device went away."""


def lent_device(name: str) -> str:
    """Return the device `SERIAL_PORTS_<NAME>` names for the port `name`.

    Raises `LookupError` when the variable is unset or empty.
    """
    variable = f"SERIAL_PORTS_{name.upper()}"
    device = os.environ.get(variable, "")
    if not device:
        raise LookupError(f"{variable} names no device for the {name} port")
    return device


def open_port(device: str, baud_rate: int, timeout: float) -> Bridge | Device:
    """Open `device` at `baud_rate`, each read waiting up to `timeout` seconds for its first byte."""
    if device.startswith(TCP_PREFIX):
        return Bridge(device.removeprefix(TCP_PREFIX), timeout)
    return Device(device, baud_rate, timeout)


class Bridge:
    """A port a bridge serves over TCP, which reports an orderly close as a read of nothing.

    A plain socket rather than pyserial's `socket://` handler, which drops the bytes a read had already gathered when
    the bridge hangs up partway through it.
    """

    def __init__(self, address: str, timeout: float) -> None:
        """Connect to the bridge at `<host>:<port>`."""
        host, _, port = address.rpartition(":")
        self._socket = socket.create_connection((host, int(port)), timeout=CONNECT_TIMEOUT_S)
        self._socket.settimeout(timeout)

    def read(self, size: int) -> bytes:
        """Return up to `size` bytes, or none when none arrived in time; raise `PortClosedError` once closed."""
        try:
            data = self._socket.recv(size)
        except TimeoutError:
            return b""
        except OSError as error:
            raise PortClosedError from error
        if not data:
            raise PortClosedError
        return data

    def write(self, data: bytes) -> None:
        """Send all of `data`."""
        self._socket.sendall(data)

    def close(self) -> None:
        """Hang up."""
        self._socket.close()


class Device:
    """A port pyserial opens: a device, or a URL one of its handlers serves, such as `loop://`."""

    def __init__(self, device: str, baud_rate: int, timeout: float) -> None:
        """Open `device` with DTR and RTS held low, as the vendors' drivers do: on some boards either resets it."""
        import serial  # noqa: PLC0415 -- the `serial` extra is optional, and only a device needs it

        self._closed = serial.SerialException
        port = serial.serial_for_url(device, baudrate=baud_rate, timeout=timeout, do_not_open=True)
        port.dtr = False
        port.rts = False
        port.open()
        self._port = port

    def read(self, size: int) -> bytes:
        """Return what has arrived, up to `size` bytes, waiting for the first; raise `PortClosedError` once closed."""
        try:
            return self._port.read(min(size, max(self._port.in_waiting, 1)))
        except self._closed as error:
            raise PortClosedError from error

    def write(self, data: bytes) -> None:
        """Send all of `data`."""
        self._port.write(data)

    def close(self) -> None:
        """Close the port."""
        self._port.close()
