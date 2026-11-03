//! What a board gives its runner: a clock and a log, and the hub.

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::bindings::wasi::logging::logging::Level;
use crate::convert::Frame;

/// Which of the guest's standard streams a write went to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Console {
    /// `wasi:cli/stdout`.
    Stdout,
    /// `wasi:cli/stderr`.
    Stderr,
}

/// The board's clock, and where the guest's log and console go, shared by every host call the guest makes.
pub trait Platform: Send + Sync {
    /// Nanoseconds since some fixed point, which only ever increase.
    fn now(&self) -> u64;

    /// Wait until [`Platform::now`] reaches `deadline`.
    fn sleep_until(&self, deadline: u64) -> impl Future<Output = ()> + Send;

    /// Record one `wasi:logging` entry from the guest.
    fn log(&self, level: Level, context: &str, message: &str);

    /// Write what the guest wrote to one of its standard streams.
    fn write(&self, console: Console, bytes: &[u8]);
}

/// The hub as the guest reaches it: a websocket, opened on the guest's `connect`, and the hub's agent storage.
///
/// Keeping the connection alive is the transport's: the hub closes one it has heard nothing on for 15 seconds, so a
/// transport pings it while the guest is busy elsewhere. The storage is the hub's `/storage/{bucket}/{key}`, which a
/// transport with the hub beside it reads from memory, and one with the hub elsewhere over HTTP.
pub trait Transport: Send {
    /// Why the websocket failed, which the guest is told as a `transport` error.
    type Error: core::fmt::Display;

    /// Open the websocket.
    ///
    /// # Errors
    ///
    /// When the websocket could not be opened.
    fn connect(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Send one data frame.
    ///
    /// # Errors
    ///
    /// When the frame could not be sent.
    fn send(&mut self, frame: Frame) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The next data frame to arrive within `timeout_ms`.
    ///
    /// `None` means none arrived in that time, which is not a failure: it is what `recv` tells the guest then.
    ///
    /// # Errors
    ///
    /// When the websocket could not be read.
    fn recv(&mut self, timeout_ms: u32) -> impl Future<Output = Result<Option<Frame>, Self::Error>> + Send;

    /// Close the websocket.
    fn close(&mut self) -> impl Future<Output = ()> + Send;

    /// What the hub stores under `key` in `bucket`.
    ///
    /// `None` is a key the bucket holds nothing under, which is not a failure: it is what `wasi:keyvalue`'s `get`
    /// tells the guest then.
    ///
    /// # Errors
    ///
    /// When the hub's storage could not be read.
    fn load(&mut self, bucket: &str, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, Self::Error>> + Send;

    /// Store `value` under `key` in `bucket` on the hub, replacing what was there.
    ///
    /// # Errors
    ///
    /// When the hub refused it, or could not be reached.
    fn store(
        &mut self,
        bucket: &str,
        key: &str,
        value: Vec<u8>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
