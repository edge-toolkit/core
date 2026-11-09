//! Host serial ports: what `et:ws-wasi/serial` lends a guest, driven directly against ports bridged over TCP.
//!
//! A test cannot plug a device in, so each port that opens here is a socket the test serves, which reads and writes
//! as a device does to the host. A device path is reached only to show an absent one fails to open.
#![cfg(test)]

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};

use et_ws_wasi_runner::HostState;
use et_ws_wasi_runner::bindings::serial_host::et::ws_wasi::serial::{Host as _, HostPort, SerialError};
use et_ws_wasi_runner::host::serial::Port;
use wasmtime::component::Resource;

/// The baud rate every open asks for; a bridged port ignores it.
const BAUD_RATE: u32 = 230_400;

/// A host lending `ports`, each a name and the device behind it.
fn host_lending(ports: &[(&str, &str)]) -> HostState {
    let mut state = HostState::new("http://127.0.0.1:9", "ws://127.0.0.1:9/ws".to_owned(), None, false);
    state.serial_ports = ports
        .iter()
        .map(|&(name, device)| (name.to_owned(), device.to_owned()))
        .collect();
    state
}

/// The device a port bridged over TCP to `address` is lent as.
fn bridge(address: SocketAddr) -> String {
    format!("tcp://{address}")
}

/// Another handle on the guest's `port`, as the guest passes one to each call.
fn borrowed(port: &Resource<Port>) -> Resource<Port> {
    Resource::new_borrow(port.rep())
}

#[tokio::test(flavor = "current_thread")]
async fn a_port_the_host_does_not_lend_is_unknown() {
    // serde-env records the prefix `motor` of `SERIAL_PORTS_MOTOR_BOARD` with an empty device, which lends nothing.
    let mut state = host_lending(&[("motor", "")]);
    for name in ["lidar", "motor"] {
        let error = state.open(name.to_owned(), BAUD_RATE).await.unwrap_err();
        assert!(matches!(&error, SerialError::UnknownPort(port) if port == name));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_absent_device_or_a_bridge_nobody_serves_fails_to_open() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let mut state = host_lending(&[("lidar", "/dev/no-such-lidar"), ("bridged", &bridge(closed))]);
    for name in ["lidar", "bridged"] {
        let error = state.open(name.to_owned(), BAUD_RATE).await.unwrap_err();
        assert!(matches!(error, SerialError::Io(_)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_bridged_port_writes_reads_waits_and_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut state = host_lending(&[("lidar", &bridge(listener.local_addr().unwrap()))]);
    let port = state.open("lidar".to_owned(), BAUD_RATE).await.unwrap();
    let (mut far, _) = listener.accept().unwrap();

    state.write(borrowed(&port), b"scan".to_vec()).await.unwrap();
    let mut command = [0; 4];
    far.read_exact(&mut command).unwrap();
    assert_eq!(&command, b"scan");

    assert_eq!(state.read(borrowed(&port), 16, 10).await.unwrap(), Vec::<u8>::new());
    far.write_all(b"data").unwrap();
    assert_eq!(state.read(borrowed(&port), 0, 1000).await.unwrap(), Vec::<u8>::new());
    assert_eq!(state.read(borrowed(&port), 16, 1000).await.unwrap(), b"data");

    drop(far);
    let error = state.read(borrowed(&port), 16, 1000).await.unwrap_err();
    assert!(matches!(error, SerialError::Closed));
    HostPort::drop(&mut state, port).await.unwrap();
}
