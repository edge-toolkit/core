//! Benchmarks one hub, held to a single CPU core, serving 25,000 agents over HTTP and again over HTTPS: connecting them
//! all, then holding them connected while each heartbeats as the native runners do.
//!
//! Run with `cargo bench -p et-ws-test-server --bench hub`. The hub has one actix worker, pinned to the first core, and
//! the agents run on a runtime whose threads are pinned to the other cores, so the agents' work never takes the hub's
//! time. macOS offers no way to pin a thread, so there the one worker thread is the whole constraint. Over HTTPS the
//! hub serves a certificate self-signed for the loopback names, which is all the agents trust.
//!
//! Both ends of every agent's socket are open in this one process, about 50,000 descriptors, so the open-file limit
//! must allow that. Agents alternate between the hub's `127.0.0.1` and `[::1]` addresses, since on macOS and Windows
//! one loopback address leaves a client about 16,000 ephemeral ports. Each agent's socket is closed with a reset rather
//! than left in `TIME_WAIT`, so one sample's ports are free again for the next.
#![expect(
    clippy::panic,
    clippy::single_call_fn,
    clippy::unwrap_used,
    reason = "criterion calls each helper once; a bench has no caller to take an error, so a failure stops the run"
)]

use std::cell::{OnceCell, RefCell};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use actix_rt::{Arbiter, ArbiterHandle};
use core_affinity::CoreId;
use cpu_time::ThreadTime;
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, SamplingMode, criterion_group, criterion_main};
use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_ws_runner_common::heartbeat_interval;
use et_ws_test_server::{HubOptions, TestServer, Transport, start_with};
use futures_util::{SinkExt as _, StreamExt as _, stream};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::task::JoinSet;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{WebSocketStream, client_async};

/// How many agents the hub serves.
const AGENTS: usize = 25_000;

/// How many agents are connecting at once while the fleet comes up.
///
/// Held under the smallest listen backlog an OS gives by default, macOS's 128 (`kern.ipc.somaxconn`): 1,000 at once
/// overflowed it, and the hub's side reset the excess mid-upgrade with `Io(Os { code: 54, kind: ConnectionReset,
/// message: "Connection reset by peer" })`.
const IN_FLIGHT: usize = 100;

/// Sockets the hub's worker holds beyond the agents', for the probe agent and those a reset leaves the hub to close.
const SPARE_CONNECTIONS: usize = 100;

/// How long one idle sample watches the hub's CPU time.
const IDLE_WINDOW: Duration = Duration::from_secs(10);

/// The id the probe agent asks for, so each of its connections reconnects one registry entry rather than adding one.
const PROBE_ID: &str = "bench-probe";

/// The byte stream an agent's websocket runs over: the TCP socket itself, or TLS on top of it.
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<Inner: AsyncRead + AsyncWrite + Unpin + Send> Stream for Inner {}

/// A registered agent's connection to the hub.
type Socket = WebSocketStream<Box<dyn Stream>>;

/// How an agent opens its byte stream to the hub.
#[derive(Clone)]
enum Dialer {
    /// Plain TCP, for a hub serving HTTP.
    Plain,
    /// TLS trusting the hub's self-signed certificate, for a hub serving HTTPS.
    Tls(TlsConnector),
}

/// The core the hub's worker runs on, and the cores left for the agents.
struct Cores {
    hub: CoreId,
    agents: Vec<CoreId>,
}

impl Cores {
    /// The first core for the hub, the rest for the agents; with only one core, the agents share it.
    fn of_this_machine() -> Self {
        let mut ids = core_affinity::get_core_ids().unwrap();
        let hub = ids.remove(0);
        let agents = if ids.is_empty() { vec![hub] } else { ids };
        Self { hub, agents }
    }
}

/// The runtime the agents run on: one thread per agent core, each pinned to its own.
fn agent_runtime(cores: &Cores) -> Runtime {
    // Each thread takes the next core in turn; there is always at least one, so the cycle never ends.
    let next_core = Mutex::new(cores.agents.clone().into_iter().cycle());
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores.agents.len())
        .enable_all()
        .on_thread_start(move || {
            let core = next_core.lock().unwrap().next().unwrap();
            // False on macOS, which cannot pin a thread; the agents then share the cores with the hub.
            let _pinned = core_affinity::set_for_current(core);
        })
        .build()
        .unwrap()
}

/// A running hub, and the arbiter of the one worker thread every agent is served on.
struct Hub {
    server: TestServer,
    worker: Arc<OnceLock<ArbiterHandle>>,
}

impl Hub {
    /// A hub over `transport` with one worker, pinned to the hub's core, holding every agent.
    ///
    /// The idle timeout stays at its default, so an agent that missed its heartbeats would be dropped and caught.
    fn start(cores: &Cores, transport: &Transport) -> Self {
        let hub_core = cores.hub;
        let worker = Arc::new(OnceLock::new());
        let worker_seen = Arc::clone(&worker);
        let mut options = HubOptions::default();
        options.workers = 1;
        options.max_connections = AGENTS + SPARE_CONNECTIONS;
        options.ipv6 = true;
        options.transport.clone_from(transport);
        options.configure = Arc::new(move |_cfg| {
            // Called on the worker thread as it starts, so this pins the thread every agent's connection is served on.
            let _pinned = core_affinity::set_for_current(hub_core);
            let _first = worker_seen.set(Arbiter::current());
        });
        Self {
            server: start_with(options),
            worker,
        }
    }

    /// The CPU time the hub's worker thread has used so far, read on that thread.
    fn cpu(&self) -> Duration {
        let (sender, used) = mpsc::channel();
        let worker = self.worker.get().unwrap();
        let spawned = worker.spawn(async move {
            let _sent = sender.send(ThreadTime::now().as_duration());
        });
        assert!(spawned, "the hub's worker has stopped");
        used.recv().unwrap()
    }

    /// Stop the hub and wait for its worker to finish, so the next hub never shares its core with this one's teardown.
    ///
    /// The hub stops without draining, which signals the worker but does not wait for it; the worker stops its arbiter
    /// as it drops, and a stopped arbiter refuses work.
    fn stop(&self, runtime: &Runtime) {
        runtime.block_on(self.server.stop());
        let worker = self.worker.get().unwrap();
        let stopped = async {
            while worker.spawn(async {}) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(30), stopped).await })
            .unwrap();
    }
}

/// Connect to `ws_url` through `dialer`, register as `agent_id` (or as the id the hub assigns), and wait for the ack.
async fn register(dialer: Dialer, ws_url: String, agent_id: Option<String>) -> Socket {
    let address: SocketAddr = ws_url
        .split_once("://")
        .and_then(|(_scheme, rest)| rest.strip_suffix("/ws"))
        .and_then(|address| address.parse().ok())
        .unwrap();
    // Fails here once the process runs out of descriptors, when the open-file limit is below about 50,000.
    let tcp = TcpStream::connect(address).await.unwrap();
    tcp.set_zero_linger().unwrap();
    let stream: Box<dyn Stream> = match dialer {
        Dialer::Plain => Box::new(tcp),
        Dialer::Tls(connector) => Box::new(connector.connect(ServerName::from(address.ip()), tcp).await.unwrap()),
    };
    let (mut socket, _response) = client_async(ws_url.as_str(), stream).await.unwrap();
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id }).unwrap();
    socket.send(Message::text(connect)).await.unwrap();
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                if matches!(
                    ServerMessage::from_text_frame(&text),
                    Ok(ServerMessage::ConnectAck { .. }),
                ) {
                    return socket;
                }
            }
            Some(Ok(_)) => {}
            other => panic!("the hub closed the socket before acking et-connect: {other:?}"),
        }
    }
}

/// Keep `socket` registered: ping every heartbeat interval, and read whatever the hub sends, until either side closes.
async fn heartbeat(mut socket: Socket) {
    let mut beat = heartbeat_interval().await;
    loop {
        tokio::select! {
            _tick = beat.tick() => {
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
            frame = socket.next() => {
                if !matches!(frame, Some(Ok(_))) {
                    return;
                }
            }
        }
    }
}

/// Every agent, each heartbeating on the agents' runtime until the fleet is dropped.
struct Fleet {
    agents: JoinSet<()>,
}

impl Fleet {
    /// Register [`AGENTS`] agents with `hub`, [`IN_FLIGHT`] at a time, alternating between its addresses.
    async fn connect(dialer: &Dialer, hub: &Hub) -> Self {
        let urls = hub.server.ws_urls.clone();
        let mut registering = stream::iter(urls.iter().cycle().take(AGENTS))
            .map(|url| tokio::spawn(register(dialer.clone(), url.clone(), None)))
            .buffer_unordered(IN_FLIGHT);
        let mut agents = JoinSet::new();
        while let Some(registered) = registering.next().await {
            let _heartbeating = agents.spawn(heartbeat(registered.unwrap()));
        }
        Self { agents }
    }

    /// Panic unless every agent is still connected: a dropped one means the hub fell behind its heartbeats.
    fn assert_all_connected(&mut self) {
        assert!(
            self.agents.try_join_next().is_none(),
            "an agent was disconnected while the hub held {AGENTS}",
        );
    }

    /// Drop every agent's socket, which resets it, and wait until they are all gone.
    async fn disband(mut self) {
        self.agents.abort_all();
        while self.agents.join_next().await.is_some() {}
    }
}

/// One `et-alive` from `probe`, answered by the hub's `et-response`.
async fn alive(probe: &mut Socket) {
    let request = serde_json::to_string(&ClientMessage::Alive {
        timestamp: String::from("bench"),
    })
    .unwrap();
    probe.send(Message::text(request)).await.unwrap();
    loop {
        match probe.next().await {
            Some(Ok(Message::Text(text))) => {
                if matches!(
                    ServerMessage::from_text_frame(&text),
                    Ok(ServerMessage::Response { .. }),
                ) {
                    return;
                }
            }
            Some(Ok(_)) => {}
            other => panic!("the hub closed the probe before answering et-alive: {other:?}"),
        }
    }
}

/// Measure `routine` as `id` in `group`, once `ready` has run untimed.
///
/// Criterion keeps what `routine` returns from being optimised away. It calls back only for a benchmark it selects,
/// so `ready` runs only when this one is measured.
fn measure<Output, Ready, Routine>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    id: &str,
    ready: Ready,
    mut routine: Routine,
) where
    Ready: Fn(),
    Routine: FnMut() -> Output,
{
    let _chained: &mut BenchmarkGroup<'_, WallTime> = group.bench_function(id, |bench| {
        ready();
        bench.iter(&mut routine);
    });
}

/// Measure `routine` as `id` in `group`, where `routine` runs the given number of iterations and times them itself.
fn measure_custom<Routine>(group: &mut BenchmarkGroup<'_, WallTime>, id: &str, mut routine: Routine)
where
    Routine: FnMut(u64) -> Duration,
{
    let _chained: &mut BenchmarkGroup<'_, WallTime> = group.bench_function(id, |bench| bench.iter_custom(&mut routine));
}

/// Every measure of a hub over `transport`, its ids prefixed with `scheme`.
///
/// The fleet comes up on a fresh hub for every sample, since a hub keeps a registry entry for each agent it has seen.
/// The held fleet is brought up only after that, as the two together would need twice the ports, and only when a
/// measure that needs it runs, so listing the benchmarks or filtering them all out connects no agent.
fn over(group: &mut BenchmarkGroup<'_, WallTime>, cores: &Cores, scheme: &str, transport: &Transport, dialer: &Dialer) {
    let runtime = agent_runtime(cores);
    let _flat: &mut BenchmarkGroup<'_, WallTime> = group.sampling_mode(SamplingMode::Flat).sample_size(10);

    measure_custom(group, &format!("{scheme}/connect_{AGENTS}"), |iterations| {
        let mut taken = Duration::ZERO;
        for _ in 0..iterations {
            let hub = Hub::start(cores, transport);
            let started = Instant::now();
            let mut fleet = runtime.block_on(Fleet::connect(dialer, &hub));
            taken = taken.saturating_add(started.elapsed());
            fleet.assert_all_connected();
            runtime.block_on(fleet.disband());
            hub.stop(&runtime);
        }
        taken
    });

    let held = OnceCell::new();
    let bring_up = || {
        let hub = Hub::start(cores, transport);
        let fleet = runtime.block_on(Fleet::connect(dialer, &hub));
        RefCell::new(Held { hub, fleet })
    };

    measure_custom(group, &format!("{scheme}/idle_cpu_{AGENTS}"), |iterations| {
        let mut held = held.get_or_init(bring_up).borrow_mut();
        let mut used = Duration::ZERO;
        for _ in 0..iterations {
            let before = held.hub.cpu();
            // Built inside the runtime: a `Sleep` registers with the timer as it is created.
            runtime.block_on(async { tokio::time::sleep(IDLE_WINDOW).await });
            used = used.saturating_add(held.hub.cpu().saturating_sub(before));
            held.fleet.assert_all_connected();
        }
        used
    });

    let _sampled: &mut BenchmarkGroup<'_, WallTime> = group.sampling_mode(SamplingMode::Auto).sample_size(100);
    let probe_url = || held.get_or_init(bring_up).borrow().hub.server.ws_url.clone();
    let probe_id = || Some(PROBE_ID.to_owned());
    let fleet_up = || {
        let _held: &RefCell<Held> = held.get_or_init(bring_up);
    };
    measure(group, &format!("{scheme}/connect_one_at_{AGENTS}"), fleet_up, || {
        runtime.block_on(register(dialer.clone(), probe_url(), probe_id()))
    });
    let probe = OnceCell::new();
    let connect_probe = || RefCell::new(runtime.block_on(register(dialer.clone(), probe_url(), probe_id())));
    let probe_up = || {
        let _probe: &RefCell<Socket> = probe.get_or_init(connect_probe);
    };
    measure(group, &format!("{scheme}/alive_at_{AGENTS}"), probe_up, || {
        runtime.block_on(alive(&mut probe.get_or_init(connect_probe).borrow_mut()));
    });

    // The probe goes before the fleet it was measured beside; neither is torn down if no measure brought it up. The
    // probe's socket is closed as its binding goes out of scope at the end of this block.
    if let Some(probe) = probe.into_inner() {
        let _closed: Socket = probe.into_inner();
    }
    if let Some(held) = held.into_inner() {
        let Held { hub, mut fleet } = held.into_inner();
        fleet.assert_all_connected();
        runtime.block_on(fleet.disband());
        hub.stop(&runtime);
    }
}

/// The hub held through the measures after the connect one, and the fleet it holds.
struct Held {
    hub: Hub,
    fleet: Fleet,
}

/// The hub over HTTP, then over HTTPS.
fn hub(criterion: &mut Criterion) {
    // Read before this thread is pinned: on Linux the cores listed are those the calling thread may run on.
    let cores = Cores::of_this_machine();
    // `block_on` polls the probe's register and alive on this thread, so it is kept off the hub's core too; the hub's
    // worker pins itself as it starts, so it does not inherit this.
    let _pinned = cores
        .agents
        .first()
        .is_some_and(|core| core_affinity::set_for_current(*core));
    let mut group = criterion.benchmark_group("hub");
    over(&mut group, &cores, "http", &Transport::Http, &Dialer::Plain);
    let (https, trusted) = Transport::self_signed();
    over(
        &mut group,
        &cores,
        "https",
        &https,
        &Dialer::Tls(TlsConnector::from(trusted)),
    );
    group.finish();
}

criterion_group!(benches, hub);
criterion_main!(benches);
