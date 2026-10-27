//! Hub lifecycle transport, durable action budgets, and backend execution.

use std::time::{Duration, Instant};

use edge_toolkit::ws::{ClientMessage, CloudEvent, MessageScope, ServerMessage};
use et_ws_runner_common::RegisteredSocket;
use fs_err as fs;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::RecoveryError;
use crate::config::{Config, Workload};
use crate::health::Session;
use crate::kubernetes::Backend;
use crate::persistence::StateStore;
use crate::policy::{Action, Budget, Decision, Health, Supervisor};

/// Restore the durable budget or create an empty budget only when no prior document exists.
pub fn load_budget(config: &Config) -> Result<Budget, RecoveryError> {
    match fs::read(&config.state_file) {
        Ok(bytes) => {
            let document: serde_json::Value = serde_json::from_slice(&bytes)?;
            let budget = document
                .get("restart_budget")
                .ok_or_else(|| RecoveryError::Invalid("state document has no restart budget".into()))?;
            Ok(serde_json::from_value(budget.clone())?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Budget::new(config.job_id.clone())),
        Err(error) => Err(error.into()),
    }
}

/// Restore a complete committed context, creating a new supervisor only when no state file exists.
pub fn load_supervisor(config: &Config) -> Result<Supervisor, RecoveryError> {
    match fs::read(&config.state_file) {
        Ok(bytes) => Supervisor::restore(config.clone(), serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Supervisor::new(config.clone(), Budget::new(config.job_id.clone()))
        }
        Err(error) => Err(error.into()),
    }
}

/// Apply one observation cycle, recording a budget reservation before requesting an external action.
pub async fn cycle<B>(
    config: &Config,
    store: &StateStore,
    supervisor: &mut Supervisor,
    backend: &mut B,
    now: u64,
) -> Result<(), RecoveryError>
where
    B: Backend,
{
    for (workload, health) in observations(config, backend, supervisor).await? {
        apply_observation(store, supervisor, backend, &workload, health, now).await?;
    }
    store.persist(supervisor.report()).await
}

/// Refresh all dependencies before yielding supervised targets eligible for policy evaluation.
async fn observations<B>(
    config: &Config,
    backend: &mut B,
    supervisor: &mut Supervisor,
) -> Result<Vec<(Workload, Health)>, RecoveryError>
where
    B: Backend,
{
    let mut observations = Vec::new();
    let mut targets = config.workloads.clone();
    for name in config.resilience.dependency_names() {
        if !targets.iter().any(|item| item.deployment == name) {
            targets.push(Workload {
                deployment: name,
                agent_id: String::default(),
                source: String::default(),
            });
        }
    }
    for workload in &targets {
        let health = match backend.health(workload).await {
            Ok(health) => health,
            Err(error) => {
                warn!(workload = workload.deployment, %error, "recovery observation unavailable");
                Health::Unknown
            }
        };
        observations.push((workload.clone(), health));
    }
    let dependencies = config.resilience.dependency_names();
    for (workload, health) in &observations {
        if dependencies.contains(&workload.deployment) {
            supervisor.dependency_observation(&workload.deployment, *health)?;
        }
    }
    observations.retain(|(workload, _health)| {
        config
            .workloads
            .iter()
            .any(|item| item.deployment == workload.deployment)
    });
    Ok(observations)
}

/// Apply current application evidence to an infrastructure observation, reserving budget before any restart.
async fn apply_observation<B>(
    store: &StateStore,
    supervisor: &mut Supervisor,
    backend: &mut B,
    workload: &Workload,
    health: Health,
    now: u64,
) -> Result<(), RecoveryError>
where
    B: Backend,
{
    if supervisor.observe(workload, health, now)? != Decision::Restart {
        return Ok(());
    }
    supervisor.reserve_restart(workload, now)?;
    store.persist(supervisor.report()).await?;
    let outcome = match backend.restart(workload).await {
        Ok(()) => Action::RestartRequested,
        Err(error) => Action::RestartFailed {
            error: error.to_string(),
        },
    };
    warn!(workload = workload.deployment, ?outcome, "recovery action");
    supervisor.action_result(workload, outcome)
}

/// Run hub transport and supervision concurrently so backend operations never block WebSocket heartbeats.
#[expect(
    clippy::integer_division_remainder_used,
    reason = "tokio::select! expands to remainder operations"
)]
pub async fn run<B>(config: Config, mut backend: B) -> Result<(), RecoveryError>
where
    B: Backend,
{
    let session = Session::acquire(&config)?;
    #[cfg(unix)]
    let termination = {
        let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        async move {
            let _signal = signals.recv().await;
        }
    };
    #[cfg(not(unix))]
    let termination = std::future::pending::<()>();
    let store = session.state_store(config.state_file.clone());
    let mut supervisor = load_supervisor(&config)?;
    // Fail closed on an unwritable budget path before registering or inspecting deployments.
    store.persist(supervisor.report()).await?;
    let started = Instant::now();
    let outcome = tokio::select! {
        result = supervise(&config, &mut supervisor, &mut backend, &store, &session, started) => result,
        signal = tokio::signal::ctrl_c() => { signal?; Ok(()) },
        () = termination => Ok(()),
    };
    supervisor.transport_lost(supervisor.elapsed(started.elapsed().as_secs()), "supervision stopped");
    store.persist(supervisor.report()).await?;
    outcome
}

/// Keep policy state and its ownership lease while re-establishing transport; persistence errors remain fatal.
#[expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "connection ownership is separate from shutdown; tokio::select! expands to remainder operations"
)]
async fn supervise<B>(
    config: &Config,
    supervisor: &mut Supervisor,
    backend: &mut B,
    store: &StateStore,
    session: &Session,
    started: Instant,
) -> Result<(), RecoveryError>
where
    B: Backend,
{
    loop {
        let socket = Box::pin(reconnect(config, supervisor, store, session, started)).await?;
        supervisor.transport_connected(supervisor.elapsed(started.elapsed().as_secs()));
        store.persist(supervisor.report()).await?;
        let (sender, receiver) = mpsc::channel::<(String, CloudEvent)>(256);
        let outcome = tokio::select! {
            result = hub(socket, sender) => result,
            result = control(config, supervisor, backend, store, session, receiver, started) => return result,
        };
        if !config.resilience.reconnect {
            return outcome;
        }
        let reason = outcome.map_or_else(|error| error.to_string(), |()| "hub transport ended".into());
        warn!(%reason, "recovery transport unavailable; interventions suspended");
        supervisor.transport_lost(supervisor.elapsed(started.elapsed().as_secs()), &reason);
        store.persist(supervisor.report()).await?;
    }
}

/// Bound registration attempts and maintain established observer health while recording unavailable evidence.
#[expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "registration is separate from connected observation; tokio::select! expands to remainder operations"
)]
async fn reconnect(
    config: &Config,
    supervisor: &mut Supervisor,
    store: &StateStore,
    session: &Session,
    started: Instant,
) -> Result<RegisteredSocket, RecoveryError> {
    if !config.resilience.reconnect {
        return connect(config).await;
    }
    loop {
        let attempt = connect(config);
        tokio::pin!(attempt);
        let mut interval = tokio::time::interval(Duration::from_secs(config.poll_seconds));
        loop {
            tokio::select! {
                result = &mut attempt => match result {
                    Ok(socket) => return Ok(socket),
                    Err(error @ RecoveryError::Invalid(_)) => return Err(error),
                    Err(error) => {
                        supervisor.transport_lost(supervisor.elapsed(started.elapsed().as_secs()), &error.to_string());
                        store.persist(supervisor.report()).await?;
                        break;
                    }
                },
                _ = interval.tick() => {
                    supervisor.transport_lost(supervisor.elapsed(started.elapsed().as_secs()), "hub registration");
                    store.persist(supervisor.report()).await?;
                    session.maintain().await?;
                }
            }
        }
    }
}

/// Register the configured supervisor identity before accepting lifecycle evidence.
async fn connect(config: &Config) -> Result<RegisteredSocket, RecoveryError> {
    let (socket, identity, _status) = et_ws_runner_common::connect_and_register(
        &config.ws_url,
        Some(config.agent_id.clone()),
        Some(Duration::from_secs(config.resilience.connect_timeout_seconds)),
    )
    .await?;
    if identity != config.agent_id {
        return Err(RecoveryError::Invalid(
            "hub did not assign the configured supervisor identity".into(),
        ));
    }
    info!(job = config.job_id, mode = ?config.mode, "recovery supervisor registered");
    Ok(socket)
}

/// Consume evidence and run bounded observation cycles while the hub handles transport independently.
#[expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "named control loop separates shutdown ownership; tokio::select! expands to remainder operations"
)]
async fn control<B>(
    config: &Config,
    supervisor: &mut Supervisor,
    backend: &mut B,
    store: &StateStore,
    session: &Session,
    mut receiver: mpsc::Receiver<(String, CloudEvent)>,
    started: Instant,
) -> Result<(), RecoveryError>
where
    B: Backend,
{
    let mut interval = tokio::time::interval(Duration::from_secs(config.poll_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            item = receiver.recv() => {
                let (identity, event) = item
                    .ok_or_else(|| RecoveryError::Invalid("hub event channel closed".into()))?;
                accept(supervisor, &identity, &event, started);
            }
            _ = interval.tick() => {
                poll(config, supervisor, backend, store, &mut receiver, started).await?;
                session.advance().await?;
            }
        }
    }
}

/// Apply queued progress before evaluating failures and persist the completed observation cycle.
#[expect(
    clippy::single_call_fn,
    reason = "one poll keeps observation, queued evidence and persistence in order"
)]
async fn poll<B>(
    config: &Config,
    supervisor: &mut Supervisor,
    backend: &mut B,
    store: &StateStore,
    receiver: &mut mpsc::Receiver<(String, CloudEvent)>,
    started: Instant,
) -> Result<(), RecoveryError>
where
    B: Backend,
{
    for (workload, health) in observations(config, backend, supervisor).await? {
        // Apply queued progress without letting a continuous producer starve observation and persistence.
        for _ in 0..receiver.len() {
            if let Ok((identity, event)) = receiver.try_recv() {
                accept(supervisor, &identity, &event, started);
            }
        }
        apply_observation(
            store,
            supervisor,
            backend,
            &workload,
            health,
            supervisor.elapsed(started.elapsed().as_secs()),
        )
        .await?;
    }
    store.persist(supervisor.report()).await
}

/// Log rejected lifecycle evidence without stopping supervision of valid workloads.
fn accept(supervisor: &mut Supervisor, identity: &str, event: &CloudEvent, started: Instant) {
    if let Err(error) = supervisor.event(identity, event, supervisor.elapsed(started.elapsed().as_secs())) {
        warn!(%error, "ignored recovery evidence");
    }
}

/// Receive direct `CloudEvents` through Core's existing acknowledged agent-message transport.
#[expect(
    clippy::integer_division_remainder_used,
    reason = "tokio::select! expands to remainder operations"
)]
pub async fn hub(
    mut socket: RegisteredSocket,
    sender: mpsc::Sender<(String, CloudEvent)>,
) -> Result<(), RecoveryError> {
    let mut heartbeat = et_ws_runner_common::heartbeat_interval().await;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => socket.send(Message::Ping(Vec::new().into())).await?,
            frame = socket.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(ServerMessage::AgentMessage { from_agent_id, message_id, scope: MessageScope::Direct,
                        message, .. }) = ServerMessage::from_text_frame(&text) {
                        if let Ok(event) = serde_json::from_value::<CloudEvent>(message) {
                            enqueue(&mut socket, &sender, (from_agent_id, event), &mut heartbeat).await?;
                        }
                        let acknowledgement = ClientMessage::MessageAck { message_id };
                        socket.send(Message::Text(serde_json::to_string(&acknowledgement)?.into())).await?;
                    }
                }
                Some(Ok(Message::Ping(bytes))) => socket.send(Message::Pong(bytes)).await?,
                Some(Ok(Message::Close(_))) | None => {
                    return Err(RecoveryError::Invalid("hub connection closed".into()));
                }
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(_)) => {}
            }
        }
    }
}

/// Retain pending evidence until queue admission, sending keepalive pings while data reads are backpressured.
#[expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "separate bounded delivery from frame handling; tokio::select! expands to remainder operations"
)]
async fn enqueue(
    socket: &mut RegisteredSocket,
    sender: &mpsc::Sender<(String, CloudEvent)>,
    evidence: (String, CloudEvent),
    heartbeat: &mut tokio::time::Interval,
) -> Result<(), RecoveryError> {
    let delivery = sender.send(evidence);
    tokio::pin!(delivery);
    loop {
        tokio::select! {
            result = &mut delivery => return result.map_err(|error| {
                RecoveryError::Invalid(format!("recovery evidence queue unavailable: {error}"))
            }),
            _ = heartbeat.tick() => socket.send(Message::Ping(Vec::new().into())).await?,
        }
    }
}
