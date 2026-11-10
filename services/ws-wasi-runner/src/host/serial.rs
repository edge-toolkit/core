//! Hosts `et:ws-wasi/serial`: the serial ports the runner's configuration lends its guests, each under a name.
//!
//! A port is a device the operating system serves, opened at the baud rate the guest asks for, or one a bridge such
//! as ser2net serves over TCP, named `tcp://<host>:<port>`. Reads and writes block, so each runs on tokio's blocking
//! pool on a handle cloned from the guest's, leaving the runtime free while a read waits out its timeout.

use std::io::{ErrorKind, Read as _, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use wasmtime::component::Resource;

use crate::HostState;
use crate::bindings::serial_host::et::ws_wasi::serial::{Host, HostPort, SerialError};

/// The prefix that names a port bridged over TCP rather than a device.
const TCP_PREFIX: &str = "tcp://";

/// The shortest a read waits: a socket cannot be given a timeout of zero.
const SHORTEST_WAIT: Duration = Duration::from_millis(1);

/// The most bytes one read returns, 64 KiB whatever the guest asks for.
///
/// A guest therefore cannot make the host allocate at will.
const MOST_READ: usize = 0x0001_0000;

/// A port a guest opened.
#[non_exhaustive]
pub enum Port {
    /// A device the operating system serves, such as `/dev/ttyUSB0` or `COM3`.
    Device(Box<dyn serialport::SerialPort>),
    /// A port a bridge serves over TCP.
    Tcp(TcpStream),
}

impl Port {
    /// Open the port `device` names, at `baud_rate` when it is a device.
    #[expect(
        clippy::single_call_fn,
        reason = "the blocking half of `open`, run on the blocking pool"
    )]
    fn open(device: &str, baud_rate: u32) -> Result<Self, SerialError> {
        Ok(match device.strip_prefix(TCP_PREFIX) {
            Some(address) => Self::Tcp(TcpStream::connect(address)?),
            None => Self::Device(serialport::new(device, baud_rate).open()?),
        })
    }

    /// Another handle on the same port, for a blocking call to own.
    fn try_clone(&self) -> Result<Self, SerialError> {
        Ok(match self {
            Self::Device(device) => Self::Device(device.try_clone()?),
            Self::Tcp(stream) => Self::Tcp(stream.try_clone()?),
        })
    }

    /// The bytes that have arrived, at most `max`, waiting up to `timeout` for the first.
    fn read(mut self, max: usize, timeout: Duration) -> Result<Vec<u8>, SerialError> {
        if max == 0 {
            return Ok(Vec::new());
        }
        let timeout = timeout.max(SHORTEST_WAIT);
        let mut buffer = vec![0; max.min(MOST_READ)];
        let outcome = match &mut self {
            Self::Device(device) => {
                device.set_timeout(timeout)?;
                device.read(&mut buffer)
            }
            Self::Tcp(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.read(&mut buffer)
            }
        };
        match outcome {
            Ok(0) => Err(SerialError::Closed),
            Ok(count) => {
                buffer.truncate(count);
                Ok(buffer)
            }
            // A socket's timeout is `WouldBlock` on Unix and `TimedOut` on Windows; a device's is `TimedOut`.
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    /// Send all of `bytes`.
    fn write(mut self, bytes: &[u8]) -> Result<(), SerialError> {
        match &mut self {
            Self::Device(device) => device.write_all(bytes)?,
            Self::Tcp(stream) => stream.write_all(bytes)?,
        }
        Ok(())
    }
}

impl Host for HostState {
    async fn open(&mut self, name: String, baud_rate: u32) -> Result<Resource<Port>, SerialError> {
        let Some(device) = self
            .serial_ports
            .get(&name)
            .filter(|device| !device.is_empty())
            .cloned()
        else {
            return Err(SerialError::UnknownPort(name));
        };
        let port = tokio::task::spawn_blocking(move || Port::open(&device, baud_rate)).await??;
        Ok(self.resource_table.push(port)?)
    }
}

impl HostPort for HostState {
    async fn read(&mut self, self_: Resource<Port>, max: u32, timeout_ms: u32) -> Result<Vec<u8>, SerialError> {
        let handle = self.resource_table.get(&self_)?.try_clone()?;
        let max = usize::try_from(max).unwrap_or(MOST_READ);
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        tokio::task::spawn_blocking(move || handle.read(max, timeout)).await?
    }

    async fn write(&mut self, self_: Resource<Port>, bytes: Vec<u8>) -> Result<(), SerialError> {
        let handle = self.resource_table.get(&self_)?.try_clone()?;
        tokio::task::spawn_blocking(move || handle.write(&bytes)).await?
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the generated HostPort trait declares every method async; the impl cannot drop it"
    )]
    async fn drop(&mut self, rep: Resource<Port>) -> wasmtime::Result<()> {
        let _closed: Port = self.resource_table.delete(rep)?;
        Ok(())
    }
}
