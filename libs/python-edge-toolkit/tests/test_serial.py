"""The serial ports a native Python ws-module reads: lent by name, over a bridge's TCP or through pyserial."""

import socket

import pytest
from edge_toolkit.serial import CONNECT_TIMEOUT_S, Bridge, Device, PortClosedError, lent_device, open_port

TIMEOUT = 0.05


def test_a_port_is_lent_by_its_variable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SERIAL_PORTS_LIDAR", "/dev/ttyUSB0")
    assert lent_device("lidar") == "/dev/ttyUSB0"


@pytest.mark.parametrize("value", [None, ""])
def test_a_port_no_variable_names_is_not_lent(monkeypatch: pytest.MonkeyPatch, value: str | None) -> None:
    if value is None:
        monkeypatch.delenv("SERIAL_PORTS_LIDAR", raising=False)
    else:
        monkeypatch.setenv("SERIAL_PORTS_LIDAR", value)
    with pytest.raises(LookupError, match="SERIAL_PORTS_LIDAR names no device for the lidar port"):
        lent_device("lidar")


def test_a_bridge_writes_reads_waits_and_closes() -> None:
    with socket.create_server(("127.0.0.1", 0)) as server:
        host, port = server.getsockname()[:2]
        bridge = open_port(f"tcp://{host}:{port}", 230_400, TIMEOUT)
        assert isinstance(bridge, Bridge)
        far, _ = server.accept()
        with far:
            bridge.write(b"scan")
            assert far.recv(16) == b"scan"
            assert bridge.read(16) == b""
            far.sendall(b"data")
            assert bridge.read(16) == b"data"
        with pytest.raises(PortClosedError):
            bridge.read(16)
        bridge.close()
        with pytest.raises(PortClosedError):
            bridge.read(16)


def test_a_bridge_waits_longer_to_connect_than_to_read(monkeypatch: pytest.MonkeyPatch) -> None:
    connected: list[tuple[float, socket.socket]] = []
    create_connection = socket.create_connection

    def recording(address: tuple[str, int], timeout: float) -> socket.socket:
        connection = create_connection(address, timeout=timeout)
        connected.append((timeout, connection))
        return connection

    monkeypatch.setattr(socket, "create_connection", recording)
    with socket.create_server(("127.0.0.1", 0)) as server:
        host, port = server.getsockname()[:2]
        bridge = open_port(f"tcp://{host}:{port}", 230_400, TIMEOUT)
        [(connect_timeout, connection)] = connected
        assert connect_timeout == CONNECT_TIMEOUT_S
        assert connection.gettimeout() == TIMEOUT
        bridge.close()


def test_a_device_writes_reads_waits_and_closes() -> None:
    device = open_port("loop://", 1_000_000, TIMEOUT)
    assert isinstance(device, Device)
    device.write(b"abc")
    assert device.read(2) == b"ab"
    assert device.read(16) == b"c"
    assert device.read(16) == b""
    device.close()
    with pytest.raises(PortClosedError):
        device.read(16)
