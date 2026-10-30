//! The NATS backplane several hubs share their agents through, from this hub's side.

use std::pin::Pin;

use actix_web::{HttpRequest, web};
pub use et_hub_backplane::{Backplane, BackplaneConfig, BackplaneError};
use et_hub_backplane::{Frame, Inbound};
use futures_util::{Stream, StreamExt as _};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::hub::SessionMessage;
use crate::server::WsAgentRegistry;

impl From<Frame> for SessionMessage {
    fn from(value: Frame) -> Self {
        match value {
            Frame::Text(text) => Self::Text(text),
            Frame::Binary(bytes) => Self::Binary(bytes),
        }
    }
}

/// The backplane the app serving `req` joined.
///
/// An app that never joined one is a local hub.
pub(crate) fn backplane_of(req: &HttpRequest) -> Backplane {
    req.app_data::<web::Data<Backplane>>()
        .map_or(Backplane::Local, |backplane| backplane.get_ref().clone())
}

/// Start delivering what other hubs send this hub's agents; a no-op for a `Local` backplane.
///
/// Returns once the subscription is live and the shared blocklist is in `registry`, so nothing another hub sends or
/// blocks afterwards can be missed. Delivery then runs on a task of the current actix runtime until the
/// [`BackplaneDelivery`] returned is stopped.
///
/// `max_frame_size` is the largest frame this hub accepts. A broker whose `max_payload` is smaller cannot carry such a
/// frame to another hub, so that mismatch is warned about here, once, rather than left to surface as a failed relay.
pub async fn start_backplane(
    backplane: &Backplane,
    registry: &WsAgentRegistry,
    max_frame_size: usize,
) -> Result<BackplaneDelivery, BackplaneError> {
    let Backplane::Nats(nats) = backplane else {
        return Ok(BackplaneDelivery(DeliveryState::Local));
    };
    warn_if_broker_payload_too_small(nats.max_payload(), max_frame_size);
    let inbox = Box::pin(nats.subscribe().await?);
    for ip in nats.blocked_ips().await? {
        let _newly = registry.blocklist.block(ip);
    }
    let (stop, stopped) = oneshot::channel::<()>();
    let task = actix_web::rt::spawn(deliver_until_stopped(inbox, stopped, registry.clone()));
    info!("Hub {} joined the NATS backplane", nats.hub_id());
    Ok(BackplaneDelivery(DeliveryState::Nats { stop, task }))
}

/// Say, at startup, when the broker cannot carry a frame as large as this hub accepts.
#[expect(
    clippy::single_call_fn,
    reason = "the startup check of start_backplane, kept out of its body"
)]
fn warn_if_broker_payload_too_small(max_payload: usize, max_frame_size: usize) {
    if max_payload < max_frame_size {
        let effect = "a larger frame reaches only the agents of the hub it arrived at";
        warn!("NATS max_payload {max_payload} < WS_MAX_FRAME_SIZE {max_frame_size}; raise the broker's: {effect}");
    }
}

/// Hand each frame `inbox` yields to the local agents it is for, until `stopped` fires or the inbox ends.
#[expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "the delivery task start_backplane spawns; tokio::select! uses % internally"
)]
async fn deliver_until_stopped(
    mut inbox: Pin<Box<impl Stream<Item = Inbound>>>,
    mut stopped: oneshot::Receiver<()>,
    registry: WsAgentRegistry,
) {
    loop {
        tokio::select! {
            _stop = &mut stopped => break,
            inbound = inbox.next() => match inbound {
                Some(inbound) => deliver_inbound(&registry, inbound),
                None => break,
            },
        }
    }
}

/// Hand one frame from another hub to the local agents it is for.
#[expect(
    clippy::single_call_fn,
    reason = "the per-frame step of the delivery loop, kept out of the select!"
)]
fn deliver_inbound(registry: &WsAgentRegistry, inbound: Inbound) {
    match inbound {
        Inbound::Direct { to_agent_id, frame } => {
            if let Some(session) = registry.agent_session(&to_agent_id) {
                let _sent = session.send(frame.into());
            }
        }
        Inbound::Broadcast { from_agent_id, frame } => {
            for (_, session) in registry.connected_sessions(&from_agent_id) {
                let _sent = session.send(frame.clone().into());
            }
        }
        Inbound::Blocked { ip } => {
            if registry.blocklist.block(ip) {
                info!("Blocked IP {ip} on the backplane's word");
            }
        }
    }
}

/// This hub's delivery of what other hubs send its agents, running until [`BackplaneDelivery::stop`].
#[must_use = "a delivery left to be dropped with the runtime aborts the process on Windows; call stop"]
#[non_exhaustive]
pub struct BackplaneDelivery(DeliveryState);

/// Whether there is a delivery task to stop.
enum DeliveryState {
    Local,
    Nats {
        stop: oneshot::Sender<()>,
        task: JoinHandle<()>,
    },
}

impl BackplaneDelivery {
    /// Stop delivering, and wait until the backplane subscriptions are closed.
    ///
    /// Call it while the runtime is still running. async-nats unsubscribes by spawning a task when a subscription is
    /// dropped, so a subscription left to be dropped with the runtime panics with
    ///
    ///     there is no reactor running, must be called from the context of a Tokio 1.x runtime
    ///
    /// from `async-nats-0.50.0/src/lib.rs:1424`, and on Windows that panic in a destructor aborts the process with
    /// `0xc0000409`. Observed on commit
    /// <https://github.com/edge-toolkit/core/commit/92a41747e7f6079494da3e6e351a2f2dba3979b2> at
    /// <https://github.com/edge-toolkit/core/actions/runs/39220980233/job/117955940089>.
    pub async fn stop(self) {
        if let DeliveryState::Nats { stop, task } = self.0 {
            // Either the task already ended or it is told to now; both leave only the join to wait for.
            let _sent = stop.send(());
            if let Err(error) = task.await {
                warn!("The backplane delivery task ended abnormally: {error}");
            }
        }
    }
}

/// Mark every agent connected to this hub as gone from the shared directory, ahead of the hub shutting down.
///
/// A stopping server drops its connection tasks rather than letting each one run its own disconnect, so without this
/// the directory would go on placing those agents on a hub that is no longer there. A no-op for `Local`.
pub async fn withdraw_local_agents(backplane: &Backplane, registry: &WsAgentRegistry) {
    let Backplane::Nats(nats) = backplane else {
        return;
    };
    for (agent_id, _session) in registry.connected_sessions("") {
        if let Err(error) = nats.withdraw(&agent_id).await {
            warn!("Failed to withdraw agent {agent_id} from the backplane: {error}");
        }
    }
}
