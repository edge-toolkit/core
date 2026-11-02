//! [`super::net`]'s sockets on a host: tokio's, read and written as a board's are.
//!
//! What a firmware's host build drives its guests' links with, so it reaches a hub elsewhere as the board would.

use std::net::SocketAddr;

use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::net::tcp::{ReadHalf, WriteHalf};

use super::net::{Connector, NetError, Split};

/// A tokio socket, or half of one, read and written as a board's socket is.
#[derive(Debug)]
pub struct TokioIo<T>(T);

impl<T> ErrorType for TokioIo<T> {
    type Error = ErrorKind;
}

impl<T: AsyncRead + Unpin> Read for TokioIo<T> {
    /// Read what the socket has into `buf`, its error reduced to its kind, as a board's are.
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ErrorKind> {
        match self.0.read(buf).await {
            Ok(read) => Ok(read),
            Err(error) => Err(error.kind().into()),
        }
    }
}

impl<T: AsyncWrite + Unpin> Write for TokioIo<T> {
    /// Write what the socket takes of `buf`, its error reduced to its kind, as a board's are.
    async fn write(&mut self, buf: &[u8]) -> Result<usize, ErrorKind> {
        match self.0.write(buf).await {
            Ok(written) => Ok(written),
            Err(error) => Err(error.kind().into()),
        }
    }

    /// Send what was written, its error reduced to its kind, as a board's are.
    async fn flush(&mut self) -> Result<(), ErrorKind> {
        match self.0.flush().await {
            Ok(()) => Ok(()),
            Err(error) => Err(error.kind().into()),
        }
    }
}

impl Split for TokioIo<TcpStream> {
    type Reader<'half> = TokioIo<ReadHalf<'half>>;
    type Writer<'half> = TokioIo<WriteHalf<'half>>;

    fn split(&mut self) -> (Self::Reader<'_>, Self::Writer<'_>) {
        let (reader, writer) = self.0.split();
        (TokioIo(reader), TokioIo(writer))
    }
}

/// Opens a tokio socket to the hub at its address each time it is asked.
#[derive(Clone, Copy, Debug)]
pub struct TokioConnector(SocketAddr);

impl TokioConnector {
    /// A connector to the hub at `address`.
    #[must_use]
    pub const fn new(address: SocketAddr) -> Self {
        Self(address)
    }
}

impl Connector for TokioConnector {
    type Socket<'socket> = TokioIo<TcpStream>;

    /// A socket to the hub, or why it could not be reached.
    async fn connect(&mut self) -> Result<TokioIo<TcpStream>, NetError> {
        match TcpStream::connect(self.0).await {
            Ok(stream) => Ok(TokioIo(stream)),
            Err(error) => Err(NetError::Connect(error.to_string())),
        }
    }
}
