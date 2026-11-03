//! Agent storage through picoserve, as the `std` service answers `/storage/{agent_id}/{filename}`, kept wherever the
//! embedder's [`Storage`] keeps it.
//!
//! A board has no filesystem, so the embedder hands in the store, usually the memory its own runners' storage buckets
//! are in, so that an agent on another machine reads what a runner on the board wrote, and the other way round. It
//! mounts [`StorageRoutes`] at [`path`] on its router.

#[cfg(not(feature = "std"))]
use alloc::{string::String, vec, vec::Vec};
use core::net::IpAddr;
use core::str::FromStr;

use edge_toolkit::ws_server::AgentRegistry;
use picoserve::ResponseSent;
use picoserve::io::Read;
use picoserve::request::Request;
use picoserve::response::{IntoResponse as _, Response, ResponseWriter, StatusCode};
use picoserve::routing::{MethodHandlerService, ParsePathSegment, parse_path_segment};

/// The largest file an agent may store, which bounds what one upload takes of a board's heap.
pub const MAX_STORED: usize = 64 * 1024;

/// Where a board keeps agents' storage: a bucket per agent id, each a map from filename to contents.
///
/// Shared by every connection the server answers at once, so a store guards its own contents.
pub trait Storage {
    /// What `agent_id`'s bucket holds under `filename`.
    ///
    /// # Errors
    ///
    /// [`NotStored`] when the bucket holds nothing under that name.
    fn get(&self, agent_id: &str, filename: &str) -> Result<Vec<u8>, NotStored>;

    /// Hold `contents` under `filename` in `agent_id`'s bucket, replacing what was there.
    fn set(&self, agent_id: &str, filename: &str, contents: Vec<u8>);
}

/// That a bucket holds nothing under the name asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "a marker with nothing to add, which every Storage outside this crate has to construct"
)]
pub struct NotStored;

/// One decoded segment of `/storage/{agent_id}/{filename}`.
///
/// Never empty, never holding a `/`, and never only dots, which would step out of its bucket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment(String);

/// That a path segment names no agent or file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct NotASegment;

impl FromStr for Segment {
    type Err = NotASegment;

    /// `segment`, already decoded, unless it names nothing a bucket holds.
    fn from_str(segment: &str) -> Result<Self, NotASegment> {
        if segment.trim_matches('.').is_empty() || segment.contains('/') {
            return Err(NotASegment);
        }
        Ok(Self(String::from(segment)))
    }
}

/// Where [`StorageRoutes`] are mounted: `/storage/{agent_id}/{filename}`, each part a [`Segment`].
#[must_use]
pub fn path() -> (&'static str, ParsePathSegment<Segment>, ParsePathSegment<Segment>) {
    ("/storage", parse_path_segment(), parse_path_segment())
}

/// The storage one connection from `client_ip` reaches, its owners as `registry` says.
pub struct StorageRoutes<'routes, S, K> {
    registry: &'routes AgentRegistry<S>,
    storage: &'routes K,
    client_ip: IpAddr,
}

impl<'routes, S, K> StorageRoutes<'routes, S, K> {
    /// `storage`, reached from `client_ip`, its owners as `registry` says.
    #[must_use]
    pub const fn new(registry: &'routes AgentRegistry<S>, storage: &'routes K, client_ip: IpAddr) -> Self {
        Self {
            registry,
            storage,
            client_ip,
        }
    }
}

impl<S: Clone + Send + 'static, K> StorageRoutes<'_, S, K> {
    /// What a `PUT` of `length` bytes into `agent_id`'s bucket is refused with, if the client may not make it.
    ///
    /// As the `std` service holds it: a blocked address may not, and otherwise only the agent that owns the bucket,
    /// while connected, from the address it connected from.
    fn refusal(&self, agent_id: &str, length: usize) -> Result<(), StatusCode> {
        if self.registry.blocklist.is_blocked(self.client_ip) {
            return Err(StatusCode::FORBIDDEN);
        }
        let Ok(owner_ip) = self.registry.agent_ip(agent_id) else {
            return Err(StatusCode::NOT_FOUND);
        };
        let connected = self.registry.agent_session(agent_id).is_some();
        if !connected || owner_ip.to_canonical() != self.client_ip.to_canonical() {
            return Err(StatusCode::FORBIDDEN);
        }
        if length > MAX_STORED {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        Ok(())
    }
}

impl<State, S, K> MethodHandlerService<State, (Segment, Segment)> for StorageRoutes<'_, S, K>
where
    S: Clone + Send + 'static,
    K: Storage,
{
    /// Answer `method` for the file `filename` in `agent_id`'s bucket.
    ///
    /// A `GET` or `HEAD` reads it, and a `PUT` stores the request's body as it.
    #[expect(
        clippy::future_not_send,
        reason = "picoserve's futures hold Cells; its executors run each connection on one thread"
    )]
    async fn call_method_handler_service<R, W>(
        &self,
        _state: &State,
        (Segment(agent_id), Segment(filename)): (Segment, Segment),
        method: &str,
        mut request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error>
    where
        R: Read,
        W: ResponseWriter<Error = R::Error>,
    {
        let status = if method.eq_ignore_ascii_case("get") || method.eq_ignore_ascii_case("head") {
            match self.storage.get(&agent_id, &filename) {
                Ok(contents) => {
                    let connection = request.body_connection.finalize().await?;
                    // A `HEAD` only asks whether the file is there, so its body is left out.
                    let body = if method.eq_ignore_ascii_case("head") {
                        &[]
                    } else {
                        contents.as_slice()
                    };
                    return Response::ok(body).write_to(connection, response_writer).await;
                }
                Err(NotStored) => StatusCode::NOT_FOUND,
            }
        } else if method.eq_ignore_ascii_case("put") {
            let length = request.body_connection.content_length();
            match self.refusal(&agent_id, length) {
                Ok(()) => {
                    let mut contents = vec![0; length];
                    let read = request.body_connection.body().reader().read_exact(&mut contents).await;
                    if read.is_ok() {
                        self.storage.set(&agent_id, &filename, contents);
                        StatusCode::OK
                    } else {
                        StatusCode::BAD_REQUEST
                    }
                }
                Err(status) => status,
            }
        } else {
            StatusCode::METHOD_NOT_ALLOWED
        };
        status
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}
